import PendingImages from "../components/PendingImages";
import MicButton from "../components/MicButton";
import SmartCards from "../components/SmartCards";
import { CampaignTrash } from "../components/TrashPanels";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import PcPicker from "../components/PcPicker";
import RunMonitor from "../components/RunMonitor";
import WebSearchPanel from "../components/WebSearchPanel";
import MemoryCompression, { AutoSummary } from "../components/MemoryCompression";
import type { PlayerCharacter } from "../bindings/PlayerCharacter";

/** 开场回合的输入标记（与后端 chronicle_gm::OPENING 一致）。 */
const OPENING = "__opening__";
import GmOrb, { type OrbState } from "../components/GmOrb";
import { api, type CampaignMemory, type CampaignStatus, type ScriptView } from "../api";
import type { Ctx } from "../App";
import type { Campaign } from "../bindings/Campaign";
import type { Script } from "../bindings/Script";
import type { TurnResult } from "../bindings/TurnResult";
import type { EventEntry } from "../bindings/EventEntry";
import { toTranscript } from "../lib/transcript";
import { describeCondition, type Names } from "../lib/dsl";
import { timelineLabel, timelineTree } from "../lib/timelines";
import { InvestigationPanel, NpcPanel, QuestPanel, WorldPanel } from "../components/GamePanels";
import { CharacterPanel, CombatPanel } from "../components/RpgPanels";
import { ConsolePanel } from "../components/ConsolePanel";
import type { Choices } from "../components/DslEditors";
import { listen } from "@tauri-apps/api/event";
import { splitForSpeech } from "../lib/speechChunks";
import type { ForkPoint } from "../bindings/ForkPoint";
import type { VerifyReport } from "../bindings/VerifyReport";
import type { TimelineComparison } from "../bindings/TimelineComparison";
import AnimatedNumber from "../components/AnimatedNumber";
import type { ShownCheck } from "../components/DiceShow";
import type { DiceMode } from "../components/DiceAsk";
import type { GameState } from "../bindings/GameState";
import type { DiceRecord } from "../bindings/DiceRecord";
import BattleScene from "../components/BattleScene";
import DiceTray from "../components/DiceTray";
import { themeFor } from "../components/OrbCanvas";
import { setLowPower, useLowPower } from "../lib/lowPower";
import { lazy, Suspense } from "react";
// 关系图谱只在游戏空间里打开时才加载（图谱组件较大）。
const NetworkEmbed = lazy(() => import("./Network"));
import type { DiceAsk as DiceAskT } from "../bindings/DiceAsk";
import type { Interaction } from "../bindings/Interaction";
import type { RitualCue } from "../bindings/RitualCue";
import InteractionHost, { resultLine } from "../components/interact/InteractionHost";
import SceneBar from "../components/interact/SceneBar";
import RitualLayer, { pickCues } from "../components/ritual/RitualLayer";
import { ambientFor, fxSettings, startAmbient, startMusic, stopMusic } from "../lib/sfx";
import { MentionThumbs, NodeGallery, ScenePictures, useIllustrated, type GalleryTarget } from "../components/SceneImages";

const TRI: Record<string, string> = { true: "条件满足", false: "条件不满足", unknown: "需检定" };

export default function GamePage({
  ctx,
  campaign,
  immersive = false,
  flyFrom = null,
  autoOpening = false,
  onExit,
}: {
  ctx: Ctx;
  campaign?: string;
  /** 独立游戏空间（全屏，没有编辑器入口与战役管理栏）。 */
  immersive?: boolean;
  /** 大厅玻璃球的位置：进入时玻璃球从那里平移到顶部中央。 */
  flyFrom?: DOMRect | null;
  /** 初始化刚完成：自动讲开场白。 */
  autoOpening?: boolean;
  /** 返回大厅。 */
  onExit?: () => void;
}) {
  const [campaigns, setCampaigns] = useState<Campaign[]>([]);
  const [scripts, setScripts] = useState<Script[]>([]);
  const [cid, setCid] = useState<string | null>(campaign ?? null);
  // 从全局搜索跳过来：切到那个战役。
  useEffect(() => {
    if (campaign) setCid(campaign);
  }, [campaign]);
  const [newName, setNewName] = useState("");
  const [newScript, setNewScript] = useState("");
  const [showArchived, setShowArchived] = useState(false);
  const rows = useMemo(() => timelineTree(campaigns, showArchived), [campaigns, showArchived]);

  useEffect(() => {
    api
      .listCampaigns(ctx.worldId)
      .then((cs) => {
        setCampaigns(cs);
        // 没选战役时，自动打开最近一个进行中、未归档的战役。
        setCid((cur) => cur ?? [...cs].reverse().find((c) => c.status === "active" && !c.archived)?.id ?? null);
      })
      .catch((e: Error) => ctx.notify(e.message, "error"));
    api.listScripts(ctx.worldId).then(setScripts).catch(() => {});
  }, [ctx.worldId, ctx.tick]);

  async function create() {
    try {
      const c = await api.createCampaign(ctx.worldId, newScript || null, newName || `战役 ${campaigns.length + 1}`);
      setNewName("");
      setCid(c.id);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  if (immersive) {
    const cur = campaigns.find((c) => c.id === cid);
    // 同一局游戏的时间线（根战役与回溯出来的分支）。
    const rootOf = (id: string | null | undefined): string | null => {
      let c = campaigns.find((x) => x.id === id);
      while (c?.parent_id) c = campaigns.find((x) => x.id === c!.parent_id) ?? undefined;
      return c?.id ?? null;
    };
    const root = rootOf(cid);
    const family = rows.filter((r) => rootOf(r.campaign.id) === root);
    return (
      <section className="game immersive">
        <header className="row game-head play-head">
          <button className="small" onClick={onExit} title="进度随时自动保存（每个回合都有检查点），随时可以回来继续" data-tour="exit-game">
            ← 保存并返回大厅
          </button>
          <strong className="play-title">{cur?.name ?? "游戏"}</strong>
          {family.length > 1 && (
            <select value={cid ?? ""} onChange={(e) => e.target.value && setCid(e.target.value)} title="切换时间线（回溯产生的分支）">
              {family.map((r) => (
                <option key={r.campaign.id} value={r.campaign.id}>
                  {timelineLabel(r)}
                </option>
              ))}
            </select>
          )}
        </header>
        {cid ? <Table key={cid} ctx={ctx} campaignId={cid} campaigns={campaigns} onSwitch={(id) => { setCid(id); ctx.bump(); }} immersive flyFrom={flyFrom} autoOpening={autoOpening} /> : <p className="hint">正在打开…</p>}
      </section>
    );
  }

  return (
    <section className="game">
      <header className="row game-head">
        <h2 style={{ margin: 0 }}>游戏</h2>
        <select value={cid ?? ""} onChange={(e) => setCid(e.target.value || null)}>
          <option value="">选择战役 / 时间线…</option>
          {rows.map((r) => (
            <option key={r.campaign.id} value={r.campaign.id}>
              {timelineLabel(r)}
            </option>
          ))}
        </select>
        <label className="check">
          <input type="checkbox" checked={showArchived} onChange={(e) => setShowArchived(e.target.checked)} />
          显示已归档
        </label>
        <span className="hint">或新建：</span>
        <select value={newScript} onChange={(e) => setNewScript(e.target.value)} data-tour="new-script">
          <option value="">自由沙盒（不使用剧本）</option>
          {scripts.map((s) => (
            <option key={s.id} value={s.id}>
              {s.title}
            </option>
          ))}
        </select>
        <input value={newName} onChange={(e) => setNewName(e.target.value)} placeholder="战役名称" />
        <button onClick={() => void create()} data-tour="new-campaign">新建战役</button>
        {cid && (
          <button
            className="small danger"
            title="移到回收站（可以恢复）；从它分出的时间线一起移走"
            onClick={() => {
              const c = campaigns.find((x) => x.id === cid);
              if (!window.confirm("把战役「" + (c?.name ?? "") + "」移到回收站？\n从它分出的时间线会一起移走，可以在回收站恢复。")) return;
              void api
                .trashCampaign(ctx.worldId, cid)
                .then(() => {
                  setCid(null);
                  ctx.notify("已移到回收站");
                  ctx.bump();
                })
                .catch((e: Error) => ctx.notify(e.message, "error"));
            }}
          >
            删除战役
          </button>
        )}
        <CampaignTrash ctx={ctx} />
      </header>
      {cid ? <Table key={cid} ctx={ctx} campaignId={cid} campaigns={campaigns} onSwitch={(id) => { setCid(id); ctx.bump(); }} /> : <p className="hint">选择或新建一个战役开始游戏。GM 由 AI 主持；骰子、规则与状态变化都由程序判定。</p>}
    </section>
  );
}

function Table({
  ctx,
  campaignId,
  campaigns,
  onSwitch,
  immersive = false,
  flyFrom = null,
  autoOpening = false,
}: {
  ctx: Ctx;
  campaignId: string;
  campaigns: Campaign[];
  onSwitch: (id: string) => void;
  immersive?: boolean;
  flyFrom?: DOMRect | null;
  autoOpening?: boolean;
}) {
  const [events, setEvents] = useState<EventEntry[]>([]);
  const [status, setStatus] = useState<CampaignStatus | null>(null);
  const [script, setScript] = useState<ScriptView | null>(null);
  const [memory, setMemory] = useState<CampaignMemory | null>(null);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  // 刚发出去、还没写进剧情的那句话（先显示出来，等存档里出现同一句再换成正式的）。
  const [pendingSay, setPendingSay] = useState<string | null>(null);
  // GM 正在讲述的那一段已经输出了多少字（讲述在 GM 框的记录里逐字出现，玻璃球不再另开一个文字面板）。
  const [typedN, setTypedN] = useState(0);
  // 剧透模式（全局设置，程序按它过滤主持人信息）。
  const [gmView, setGmViewRaw] = useState(false);
  useEffect(() => {
    api.spoilerMode().then(setGmViewRaw).catch(() => {});
  }, [ctx.tick]);
  const setGmView = (on: boolean) => {
    if (on && !window.confirm("打开剧透模式？\n会显示秘密、真相、AI 的意图、被拒绝的修改和主持人关系。自己当玩家时打开会剧透。\n这是全局设置：图谱、人物关系、搜索、档案馆也会一起显示主持人信息。")) return;
    void api
      .setSpoilerMode(on)
      .then(() => {
        setGmViewRaw(on);
        ctx.bump();
      })
      .catch((e: Error) => ctx.notify(e.message, "error"));
  };
  const [last, setLast] = useState<TurnResult | null>(null);
  const [tab, setTab] = useState<"state" | "board" | "quests" | "npcs" | "world" | "character" | "combat" | "console" | "branches" | "memory" | "timeline" | "relations">("state");
  // 卡片上的「查看详情」：切到对应页签，把右侧详情滚到眼前并闪一下（之前已经在这个页签时看起来像没反应）。
  const stateRef = useRef<HTMLDivElement>(null);
  const [flash, setFlash] = useState<string | null>(null);
  const [drawer, setDrawer] = useState(false);
  useEffect(() => {
    if (!drawer) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setDrawer(false);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [drawer]);
  const openTab = (t: string) => {
    setTab(t as typeof tab);
    setDrawer(true);
    setFlash(null);
    requestAnimationFrame(() => {
      setFlash(t);
      const el = document.querySelector<HTMLElement>(".table-body .detail .tabs");
      el?.scrollIntoView({ block: "start", behavior: "smooth" });
    });
    window.setTimeout(() => setFlash((f) => (f === t ? null : f)), 1200);
  };
  const [draft, setDraft] = useState("");
  const bottom = useRef<HTMLDivElement>(null);
  // GM 用的是本机模型还是云端模型（等待提示不同）。
  const [gmLocal, setGmLocal] = useState<boolean | null>(null);
  useEffect(() => {
    Promise.all([api.listRoutes(), api.listProviders()])
      .then(([routes, ps]) => {
        const id = routes.find(([t]) => t === "narration")?.[1] ?? routes.find(([t]) => t === "default")?.[1];
        const p = ps.find((x) => x.config.id === id);
        setGmLocal(p ? p.loopback : null);
      })
      .catch(() => {});
  }, [ctx.tick]);
  // 设置里不常用的项收在「更多设置」里。
  const [moreSet, setMoreSet] = useState(false);
  // 游戏空间：右侧图标栏默认收起，右下角「面板」按钮展开；齿轮里的设置。
  const [railOpen, setRailOpen] = useState(false);
  // GM 框的宽度：拖左边的把手自由调整（记住上次的宽度），双击把手恢复默认。
  const GM_W_DEFAULT = 460;
  const [gmW, setGmW] = useState(() => {
    try {
      const v = Number(localStorage.getItem("gm.width"));
      return v >= 300 && v <= 1000 ? v : GM_W_DEFAULT;
    } catch {
      return GM_W_DEFAULT;
    }
  });
  const [gmDrag, setGmDrag] = useState(false);
  const startGmResize = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = gmW;
    const box = (e.currentTarget.closest(".table-body") as HTMLElement | null)?.clientWidth ?? 1400;
    const max = Math.max(320, Math.min(1000, box * 0.7));
    let w = startW;
    setGmDrag(true);
    const move = (ev: PointerEvent) => {
      w = Math.round(Math.min(max, Math.max(300, startW + (startX - ev.clientX))));
      setGmW(w);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      setGmDrag(false);
      try {
        localStorage.setItem("gm.width", String(w));
      } catch {
        /* 存不下只在本次有效 */
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  const [setsOpen, setSetsOpen] = useState(false);
  // 游戏设置弹出层：点外面或按 Esc 收起。
  useEffect(() => {
    if (!setsOpen) return;
    const onDown = (e: PointerEvent) => {
      if (!(e.target as HTMLElement | null)?.closest(".play-settings, .play-settings-pop")) setSetsOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setSetsOpen(false);
    window.addEventListener("pointerdown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [setsOpen]);
  // 教程讲到面板里的东西时，自动展开面板（以及「更多设置」）。
  useEffect(() => {
    const on = (e: Event) => {
      const t = ((e as CustomEvent<string[] | undefined>).detail ?? []) as string[];
      if (t.some((x) => x === "tabs" || x.startsWith("tab-") || x === "spoiler" || x === "dice-mode" || x === "game-settings")) {
        setRailOpen(true);
        if (!immersive) setDrawer(true);
      }
      if (t.includes("spoiler") || t.includes("game-settings")) {
        setMoreSet(true);
        setSetsOpen(true);
      }
    };
    window.addEventListener("tour-step", on);
    return () => window.removeEventListener("tour-step", on);
  }, [immersive]);
  // GM 玻璃球：构思中的阶段；正在讲述的叙述（讲完才落进剧情流）；刚落下的叙述（播放落下动画）。
  const [stage, setStage] = useState<string | undefined>(undefined);
  const [speaking, setSpeaking] = useState<string | null>(null);
  // 游戏状态机：战斗 / 战后时剧情暂停，显示战斗场景。
  const [game, setGame] = useState<GameState | null>(null);
  useEffect(() => {
    api.gameState(ctx.worldId, campaignId).then(setGame).catch(() => {});
  }, [ctx.worldId, campaignId, ctx.tick]);
  useEffect(() => {
    let off: (() => void) | undefined;
    let gone = false;
    void listen<{ campaign_id: string; state: GameState }>("game-state", (e) => {
      if (e.payload.campaign_id === campaignId) setGame(e.payload.state);
    }).then((f) => (gone ? f() : (off = f)));
    return () => {
      gone = true;
      off?.();
    };
  }, [campaignId]);
  const inBattle = game?.phase === "combat" || game?.phase === "aftermath";
  // GM 玻璃球的主题色跟着剧本（和大厅一致）；背景不再用彩色光斑，改成点阵。
  const orbTheme = useMemo(() => (script ? themeFor(script.script.genre, script.script.title) : undefined), [script]);
  const playTheme = useMemo(() => {
    if (!orbTheme) return undefined;
    const rgb = (c: [number, number, number], a: number) => `rgba(${c.map((x) => Math.round(x * 255)).join(",")},${a})`;
    return { "--play-ink": rgb(orbTheme.a, 0.9), "--play-dot": rgb(orbTheme.a, 0.45) } as React.CSSProperties;
  }, [orbTheme]);
  const lowPower = useLowPower();
  const lowPowerRef = useRef(lowPower);
  lowPowerRef.current = lowPower;
  // 玩家倒地（生命 0）：只能休息、求助、用药（规则由后端检查，这里给出提示和快捷按钮）。
  const [party, setParty] = useState<{ hp: number; max: number; name: string; rule: string } | null>(null);
  useEffect(() => {
    api
      .rpgOverview(ctx.worldId, campaignId)
      .then((o) => setParty({ hp: o.sheet.hp, max: o.sheet.max_hp, name: o.sheet.name, rule: o.rules.party_death }))
      .catch(() => {});
  }, [ctx.worldId, campaignId, ctx.tick]);
  const downed = !!party && party.hp <= 0 && party.rule === "downed";
  // 骰子托盘：检定与战斗里的每一次掷骰（结果来自规则引擎，动画只是演出）。
  const [tray, setTray] = useState<DiceRecord[]>([]);
  // 沉浸式互动：等玩家亲自操作的互动（GM 提出或点场景对象）；仪式演出队列。
  const [interactions, setInteractions] = useState<Interaction[]>([]);
  const [rituals, setRituals] = useState<RitualCue[]>([]);
  const [ixKey, setIxKey] = useState(0);
  useEffect(() => {
    api
      .interactionView(ctx.worldId, campaignId)
      .then((v) => setInteractions(v.pending))
      .catch(() => {});
  }, [ctx.worldId, campaignId, ctx.tick]);
  // 背景乐与环境音（演出强度"完整"时）：战斗用战斗乐，平时按地点和天气放环境音。
  useEffect(() => {
    if (fxSettings.level !== "full" || lowPower) {
      stopMusic();
      void startAmbient("");
      return;
    }
    void startMusic(game?.phase === "combat" ? "combat" : "explore");
    return () => {
      stopMusic();
      void startAmbient("");
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [game?.phase, lowPower]);
  useEffect(() => {
    if (fxSettings.level !== "full" || lowPower) return;
    api
      .sceneView(ctx.worldId, campaignId)
      .then((s) => void startAmbient(ambientFor(s.location?.title ?? "", "")))
      .catch(() => {});
  }, [ctx.worldId, campaignId, ctx.tick, lowPower]);
  /** 剧本的调查比重：0 时不显示「调查」页签（没有剧本时按 1）。 */
  const [investigation, setInvestigation] = useState(1);
  /** 正在查看图片的节点（地点、人物或叙述里提到的设定）。 */
  const [gallery, setGallery] = useState<GalleryTarget | null>(null);
  const illustrated = useIllustrated(ctx.worldId, ctx.tick);
  // 玩家角色：undefined = 还没读到；null = 没有设定。
  const [pc, setPc] = useState<PlayerCharacter | null | undefined>(undefined);
  const [picking, setPicking] = useState(true);

  const refresh = useCallback(async () => {
    try {
      const [ev, st, mem] = await Promise.all([
        api.campaignLog(ctx.worldId, campaignId),
        api.campaignStatus(ctx.worldId, campaignId),
        api.campaignMemory(ctx.worldId, campaignId),
      ]);
      setEvents(ev);
      setStatus(st);
      api.campaignPc(ctx.worldId, campaignId).then(setPc).catch(() => setPc(null));
      setMemory(mem);
      const d = mem.summaries.find((x) => x.status === "draft");
      setDraft(d?.summary ?? "");
      if (st.campaign.script_id && !script) {
        setScript(await api.getScript(ctx.worldId, st.campaign.script_id));
        api.scriptGameProfile(ctx.worldId, st.campaign.script_id).then((p) => setInvestigation(p.weights.investigation)).catch(() => {});
      }
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }, [ctx.worldId, campaignId, script]);

  useEffect(() => {
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ctx.tick]);

  // 滚到最新一行：直接滚动剧情容器（不用 scrollIntoView——它会强制整页重新排版，长剧情时打开游戏页卡 100ms 以上）。
  // 第一次打开立即到底；之后有新内容时平滑滚动。
  const linesRef = useRef<HTMLDivElement>(null);
  // 打开页面时已有的行不播放入场动画，之后新加入的行才浮现（记下第一次显示时已有的行）。
  const initialLines = useRef<Set<string> | null>(null);
  // 贴底：剧情区始终显示最新的对话。GM 讲述时玻璃球在上面长高、剧情区变矮，NPC 回应和图片也会晚到，
  // 所以不只在新事件时滚动，而是剧情区大小或内容一变就回到底部。玩家往上翻看时不跟随，翻回底部再继续跟随。
  const stick = useRef(true);
  const autoScrollAt = useRef(0);
  const toBottom = useCallback(() => {
    const el = linesRef.current;
    if (!el) return;
    autoScrollAt.current = performance.now();
    el.scrollTop = el.scrollHeight;
  }, []);
  useEffect(() => {
    const el = linesRef.current;
    if (!el) return;
    const onScroll = () => {
      // 程序自己滚动引起的事件不算玩家翻看。
      if (performance.now() - autoScrollAt.current < 120) return;
      stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
    };
    const follow = () => {
      if (stick.current) requestAnimationFrame(toBottom);
    };
    const ro = new ResizeObserver(follow);
    ro.observe(el);
    const mo = new MutationObserver(follow);
    mo.observe(el, { childList: true, subtree: true, characterData: true });
    el.addEventListener("scroll", onScroll, { passive: true });
    el.addEventListener("load", follow, true);
    return () => {
      ro.disconnect();
      mo.disconnect();
      el.removeEventListener("scroll", onScroll);
      el.removeEventListener("load", follow, true);
    };
  }, [toBottom, campaignId]);
  // 有新事件（玩家行动、GM 回应）时一定回到底部，重新开始跟随。
  useEffect(() => {
    if (events.length === 0) return;
    stick.current = true;
    toBottom();
    const id = window.setTimeout(toBottom, 60);
    return () => clearTimeout(id);
  }, [events.length, toBottom]);
  useEffect(() => {
    if (initialLines.current == null && lines.length > 0) initialLines.current = new Set(lines.map((l, i) => `${l.seq}-${i}`));
  });

  // 本战役里才有的物品（角色的初始装备，id 以 kit- 开头）不在剧本里：从背包读名字。
  const [bagNames, setBagNames] = useState<Map<string, string>>(new Map());
  useEffect(() => {
    api
      .rpgOverview(ctx.worldId, campaignId)
      .then((o) => setBagNames(new Map(o.inventory.map((i) => [i.item.id, i.item.title]))))
      .catch(() => {});
  }, [ctx.worldId, campaignId, ctx.tick]);
  const names: Names = useMemo(() => {
    const m = new Map<string, string>(bagNames);
    for (const it of script?.items ?? []) m.set(it.id, it.title);
    for (const c of script?.cast ?? []) m.set(c.cast.node_id, c.name);
    return (id: string) => m.get(id) ?? id;
  }, [script, bagNames]);

  const lines = useMemo(() => {
    const all = toTranscript(events).filter((l) => gmView || !l.gmOnly);
    if (!speaking) return all;
    // 正在由玻璃球讲述的叙述先不进剧情流，讲完再落下（同一段文字不会同时出现两遍）。
    for (let i = all.length - 1; i >= 0; i--) {
      if (all[i].who === "gm" && all[i].text.trim() === speaking) return all.filter((_, j) => j !== i);
    }
    return all;
  }, [events, gmView, speaking]);
  // 检定：后端每掷一次骰就推送一条，玻璃球依次展开演出（结果已确定）。
  const [checkQueue, setCheckQueue] = useState<ShownCheck[]>([]);
  const checksShown = useRef(0);
  // 从大厅进来：玻璃球从大厅中间的位置平移、缩小到顶部中央（FLIP；只用 transform，交给 GPU）。
  const stageRef = useRef<HTMLDivElement>(null);
  // 存档里出现了刚发出去的那句话：去掉临时显示的那条。
  useEffect(() => {
    if (!pendingSay) return;
    const lastPlayer = [...lines].reverse().find((l) => l.who === "player");
    if (lastPlayer && lastPlayer.text.trim().endsWith(pendingSay.trim())) setPendingSay(null);
  }, [lines, pendingSay]);
  // GM 框：有新的讲述时滚到最新一段。
  const gmLogRef = useRef<HTMLDivElement>(null);
  const gmCount = lines.filter((l) => l.who === "gm" && !l.gmOnly).length;
  useEffect(() => {
    const el = gmLogRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [gmCount, typedN, speaking]);
  useEffect(() => setTypedN(0), [speaking]);
  useLayoutEffect(() => {
    const el = stageRef.current?.querySelector<HTMLElement>(".gm");
    if (!el || !flyFrom) return;
    const to = el.getBoundingClientRect();
    if (!to.width) return;
    const dx = flyFrom.left + flyFrom.width / 2 - (to.left + to.width / 2);
    const dy = flyFrom.top + flyFrom.height / 2 - (to.top + to.height / 2);
    const s = Math.max(1, flyFrom.width / Math.max(1, to.width));
    el.animate(
      [
        { transform: `translate(${dx}px, ${dy}px) scale(${s})`, opacity: 0.85 },
        { transform: "translate(0, 0) scale(1)", opacity: 1 },
      ],
      { duration: 900, easing: "cubic-bezier(.2,.9,.22,1.06)", fill: "backwards" },
    );
    // 只在进入时播一次。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  // 亲手掷骰：回合停在检定处推送 gm-dice-ask，玻璃球里显示掷骰面板，掷完把点数交回。
  const [diceMode, setDiceModeRaw] = useState<DiceMode>("auto");
  useEffect(() => {
    api.diceMode().then(setDiceModeRaw).catch(() => {});
  }, [ctx.tick]);
  const [diceAsk, setDiceAsk] = useState<DiceAskT | null>(null);
  useEffect(() => {
    let off: (() => void) | undefined;
    let gone = false;
    void listen<{ campaign_id: string; ask: DiceAskT }>("gm-dice-ask", (e) => {
      if (e.payload.campaign_id === campaignId) setDiceAsk(e.payload.ask);
    }).then((f) => (gone ? f() : (off = f)));
    return () => {
      gone = true;
      off?.();
    };
  }, [campaignId]);
  const answerDice = (faces: number[][] | null) => {
    setDiceAsk(null);
    void api.gmDiceAnswer(campaignId, faces).catch((e: Error) => ctx.notify(e.message, "error"));
  };
  useEffect(() => {
    let off: (() => void) | undefined;
    let gone = false;
    void listen<{ campaign_id: string; check: { skill: string; reason: string; detail: ShownCheck["detail"] | null } }>("gm-check", (e) => {
      const c = e.payload.check;
      if (e.payload.campaign_id !== campaignId || !c?.detail) return;
      // 3D 骰子托盘演出（低性能模式用玻璃球里的平面演出）。回合结束时结果里还会带上全部掷骰，记下已经演过的检定数，到时跳过。
      checksShown.current += 1;
      if (lowPowerRef.current) setCheckQueue((q) => [...q, { skill: c.skill, reason: c.reason, detail: c.detail! }]);
      else setTray((q) => [...q, checkToRecord(c.skill, c.reason, c.detail!)]);
    }).then((f) => (gone ? f() : (off = f)));
    return () => {
      gone = true;
      off?.();
    };
  }, [campaignId]);
  // GM 的讲述留在玻璃球里：讲完停在"查看"状态，点球或剧情流里的小条可以翻看之前的讲述。
  const gmSaid = useMemo(() => toTranscript(events).filter((l) => l.who === "gm" && !l.gmOnly && l.text.trim()).map((l) => ({ turn: l.turn, text: l.text, seq: l.seq })), [events]);
  // LATEST：总是显示最新的一段。讲完时用它，而不是"当时列表的最后一条"——讲述开始时新的讲述还没进列表，
  // 记下的下标会指向上一段（实测讲完停在 4/5）。
  const LATEST = -1;
  const [readIdx, setReadIdx] = useState<number | null>(null);
  const readAt = readIdx === LATEST ? (gmSaid.length ? gmSaid.length - 1 : null) : readIdx;
  const [unread, setUnread] = useState(false);
  const orbState: OrbState = checkQueue.length || diceAsk ? "rolling" : busy ? "thinking" : speaking ? "speaking" : readAt != null && gmSaid[readAt] ? "reading" : "idle";
  // 朗读：前端按顺序一条一条播放（后端有缓存与队列上限）。
  const [autoRead, setAutoRead] = useState(() => {
    try {
      return localStorage.getItem("autoRead") === "1";
    } catch {
      return false;
    }
  });
  const voice = useVoice(ctx);
  // 手动修改游戏状态用的选项（剧本的任务、线索、物品、人物）。
  const choices: Choices = useMemo(() => {
    const by = (k: string) => (script?.items ?? []).filter((i) => i.kind === k).map((i) => ({ id: i.id, label: i.title }));
    return { quests: by("quest"), clues: by("clue"), items: by("item"), plotNodes: [], people: (script?.cast ?? []).map((c) => ({ id: c.cast.node_id, label: c.name })) };
  }, [script]);
  // 后台的导演分析完成：提示主持人（玩家视角不提示）。
  useEffect(() => {
    let off: (() => void) | undefined;
    let gone = false;
    void listen<{ campaign_id: string; adopted: number; trigger: string }>("director-note", (e) => {
      if (e.payload.campaign_id !== campaignId) return;
      if (gmView) ctx.notify(`导演给出了新的建议${e.payload.adopted ? `（已自动采用 ${e.payload.adopted} 条）` : ""}，见「控制台」`);
      ctx.bump();
    }).then((f) => (gone ? f() : (off = f)));
    return () => {
      gone = true;
      off?.();
    };
  }, [campaignId, gmView]);

  useEffect(() => {
    let off: (() => void) | undefined;
    let gone = false;
    void listen<{ campaign_id: string; stage: string }>("gm-stage", (e) => {
      if (e.payload.campaign_id === campaignId) setStage(e.payload.stage);
    }).then((f) => (gone ? f() : (off = f)));
    return () => {
      gone = true;
      off?.();
    };
  }, [campaignId]);

  async function send(text?: string) {
    const said = (text ?? input).trim();
    if (!said || busy) return;
    const opening = said === OPENING;
    setReadIdx(null);
    checksShown.current = 0;
    // 你说的话立刻出现在对话里（不用等 GM 回完）。
    if (!opening) setPendingSay(said);
    setBusy(true);
    setStage(opening ? "opening" : undefined);
    setSpeaking(null);
    try {
      const r = await api.gmTurn(ctx.worldId, campaignId, said);
      const narr = r.narration.trim();
      // 自动朗读：先把讲述合成成语音（玻璃球显示"正在生成语音"），合成好第一段再让文字和声音一起出来。
      // 合成失败时照常显示文字。
      if (narr && autoRead) {
        setStage("voice");
        await voice.speakSynced(null, narr);
      }
      if (narr) setSpeaking(narr);
      else setUnread(false);
      if (autoRead) for (const n of r.npc_lines) voice.enqueue(n.npc_id, n.text);
      setLast(r);
      if (r.interactions?.length) setInteractions((q) => [...q.filter((x) => !r.interactions.some((n) => n.target === x.target && n.type === x.type)), ...r.interactions]);
      if (r.rituals?.length) setRituals((q) => [...q, ...pickCues(r.rituals, fxSettings.level)]);
      setIxKey((k) => k + 1);
      if (!opening && text === undefined) setInput("");
      if (r.leaks.length || r.npc_lines.some((n) => n.leaks.length)) ctx.notify("本回合的文字中可能泄露了 GM 笔记里的秘密（打开剧透模式可查看）", "error");
      if (r.entities.some((e) => e.entity_id)) ctx.notify("AI 提议了新的人物或地点，已加入本战役，确认后才写入世界（见审阅台）");
      // 游戏空间里战斗在中间的战斗场景里进行，右侧不用跳到战斗页签。
      if (r.combat?.status === "active" && !immersive) setTab("combat");
      if (r.dice?.length) {
        // 回合进行中已经实时演过的检定不再演第二遍。
        let skip = checksShown.current;
        const rest = r.dice.filter((d) => !(d.category === "check" && skip-- > 0));
        if (rest.length) setTray((q) => [...q, ...rest].slice(-24));
      }
      const news = [...r.rpg_updates.slice(-3), ...(r.travel ? [r.travel] : []), ...r.places_heard.map((p) => `听说了：${p}`), ...r.discoveries.map((d) => `新发现：${d}`), ...r.quest_updates];
      if (news.length) ctx.notify(`${r.time}　${news.join("；")}`);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
      setPendingSay(null);
      ctx.bump();
    } finally {
      setBusy(false);
      setDiceAsk(null);
    }
  }

  async function summarize() {
    try {
      await api.gmSummarize(ctx.worldId, campaignId);
      ctx.notify("已生成摘要草稿，请检查后确认");
      setTab("memory");
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  async function confirmDraft(id: string) {
    try {
      await api.confirmSummary(ctx.worldId, id, draft);
      ctx.notify("摘要已确认，之后的回合 GM 会参考它");
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  // GM 给出的可选行动：最近一次 choices 事件之后玩家还没有行动时显示。
  const gmChoices = useMemo(() => {
    for (let i = events.length - 1; i >= 0; i--) {
      const e = events[i];
      if (e.kind === "player") return [];
      if (e.kind === "choices") return (((e.payload as { items?: string[] }) ?? {}).items ?? []).slice(0, 4);
    }
    return [];
  }, [events]);
  const started = events.some((e) => e.kind === "narration" || e.kind === "player");
  const atStart = !!status && status.campaign.turn === 0 && !started && status.campaign.status !== "ended";
  const showPicker = atStart && picking && pc === null && !busy;
  const showOpening = atStart && !busy && (pc != null || !picking);
  // 初始化刚完成（从大厅进来）：角色已经选好，自动讲开场白（只触发一次）。
  const autoOpened = useRef(false);
  useEffect(() => {
    if (!autoOpening || autoOpened.current || !status || !atStart || busy || !pc) return;
    // 等玻璃球平移到位再开口。标记在真正发出时才置上：等待期间状态刷新会重新安排，而不是丢掉。
    const t = window.setTimeout(() => {
      if (autoOpened.current) return;
      autoOpened.current = true;
      void send(OPENING);
    }, flyFrom ? 950 : 200);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoOpening, status, atStart, busy, pc]);

  const st = status?.state;
  // 游戏空间：设置收进齿轮菜单，面板变成右侧图标栏 + 抽屉。
  const settingsControls = (
    <div className="game-settings">
      <label className="check" title="每回合结束后自动朗读叙述和人物台词（需要在「模型服务」设置语音服务）">
        <input
          type="checkbox"
          checked={autoRead}
          onChange={(e) => {
            setAutoRead(e.target.checked);
            try {
              localStorage.setItem("autoRead", e.target.checked ? "1" : "0");
            } catch {
              /* 浏览器存储不可用时只在本次有效 */
            }
            if (!e.target.checked) voice.stop();
          }}
        />
        自动朗读
      </label>
      {voice.playing && <button className="small link" onClick={() => voice.stop()}>停止朗读</button>}
      <label className="check" title="检定时谁来掷骰：程序自动掷；轮到检定时由你点击掷骰；或者掷自己的实体骰子再输入点数（全局设置）">
        <span className="set-name">掷骰</span>
        <select
          value={diceMode}
          onChange={(e) => {
            const m = e.target.value as DiceMode;
            void api
              .setDiceMode(m)
              .then(() => setDiceModeRaw(m))
              .catch((err: Error) => ctx.notify(err.message, "error"));
          }}
          data-tour="dice-mode"
        >
          <option value="auto">程序自动掷</option>
          <option value="click">我来点击掷骰</option>
          <option value="physical">掷实体骰子，输入点数</option>
        </select>
      </label>
      <details className="more-set" open={moreSet} onToggle={(e) => setMoreSet((e.target as HTMLDetailsElement).open)}>
        <summary>更多设置</summary>
        <label className="check" data-tour="spoiler" title="显示秘密、真相、AI 的意图和被拒绝的修改。自己当主持人时用；自己当玩家时别开。全局设置。">
          <input type="checkbox" checked={gmView} onChange={(e) => { setGmView(e.target.checked); if (!e.target.checked && tab === "console") setTab("state"); }} />
          剧透模式
        </label>
        <label className="check" title="关掉着色器玻璃球、3D 骰子、粒子和大部分动画（老电脑或用电池时；全局设置）">
        <input type="checkbox" checked={lowPower} onChange={(e) => void setLowPower(e.target.checked).catch((er: Error) => ctx.notify(er.message, "error"))} />
        低性能模式
      </label>
        <RunMonitor refreshKey={events.length} openDefault={false} />
        <WebSearchPanel notify={ctx.notify} />
      </details>
    </div>
  );
  const tabList = (
    [
      ["state", "状态", "📋"],
      ["branches", "剧情分支", "🌿"],
      ["memory", "记忆", "🧠"],
      ["board", immersive ? "线索" : "调查", "🔍"],
      ["quests", "任务", "📜"],
      ["npcs", immersive ? "NPC 档案" : "人物", "👥"],
      ["relations", "关系图谱", "🕸️"],
      ["world", immersive ? "地图" : "世界", "🗺️"],
      ["character", immersive ? "背包与属性" : "角色", "🎒"],
      ["combat", "战斗", "⚔️"],
      ["timeline", immersive ? "剧情历史" : "时间线", "🕰️"],
      ["console", "控制台", "🎛️"],
    ] as const
  )
    // 游戏空间只留玩家用得到的；剧情分支、控制台属于主持人工具，打开剧透模式才显示。关系图谱只在游戏空间里（创作模式有单独的图谱页）。
    .filter(([k]) => (k !== "console" || gmView) && (k !== "board" || investigation > 0 || tab === "board") && (k !== "branches" || !immersive || gmView) && (k !== "relations" || immersive) && (k !== "combat" || !immersive));
  const tabTitle = tabList.find(([k]) => k === tab)?.[1] ?? "";
  return (
    <div
      className={`table-body${immersive ? ` play-layout${drawer ? " drawer-open" : ""}${railOpen ? " rail-open" : ""}` : ""}${gmDrag ? " gm-resizing" : ""}`}
      style={{ ...(immersive ? playTheme : {}), "--gm-w": `${gmW}px` } as React.CSSProperties}
    >
      {immersive && railOpen && (
        <nav className="play-rail" aria-label="面板">
          {tabList.map(([k, v, ic]) => (
            <button
              key={k}
              className={drawer && tab === k ? "on" : ""}
              onClick={() => {
                if (drawer && tab === k) setDrawer(false);
                else {
                  setTab(k);
                  setDrawer(true);
                }
              }}
              title={v}
              data-tour={`tab-${k}`}
            >
              <span className="ic">{ic}</span>
              <span className="lb">{v}</span>
            </button>
          ))}
          {/* 齿轮只是开关；弹出层放在图标栏外面（图标栏有毛玻璃效果，里面的弹出层会被裁掉，看不到）。 */}
          <div className={`play-settings${setsOpen ? " open" : ""}`}>
            <button type="button" className="play-settings-btn" data-tour="game-settings" title="游戏设置" aria-expanded={setsOpen} onClick={() => setSetsOpen((o) => !o)}>
              <span className="gear">⚙️</span>
            </button>
          </div>
        </nav>
      )}
      {immersive && railOpen && setsOpen && (
        <div className="play-settings-pop" role="dialog" aria-label="游戏设置">
          <div className="pop-head">
            <strong>游戏设置</strong>
            <button type="button" className="link" onClick={() => setSetsOpen(false)} title="收起（Esc）">
              ✕
            </button>
          </div>
          {settingsControls}
        </div>
      )}
      <div className={`transcript card${immersive ? " immersive" : ""}`}>
        {inBattle && game && (
          <BattleScene
            ctx={ctx}
            campaignId={campaignId}
            state={game}
            lowPower={lowPower}
            onDice={(d) => setTray((q) => [...q, ...d].slice(-24))}
            onNarration={(t) => {
              setReadIdx(null);
              setSpeaking(t);
            }}
            onChanged={() => {
              void refresh();
              ctx.bump();
            }}
          />
        )}
        <DiceTray queue={tray} lowPower={lowPower} onDone={(n) => setTray((q) => q.slice(n))} />
        <div className="lines" ref={linesRef} style={inBattle ? { display: "none" } : undefined}>
          <ScenePictures ctx={ctx} campaignId={campaignId} refreshKey={events.length + ctx.tick} onOpen={setGallery} />
          {status?.current && (
            <div className="scene">
              <strong>{status.current.title}</strong>
              {status.current.player_text && <div>{status.current.player_text}</div>}
              {gmView && status.current.gm_text && <div className="gm">GM 笔记：{status.current.gm_text}</div>}
            </div>
          )}
          {!busy && status?.incomplete && (
            <div className="line who-system warn">
              上次程序意外退出，回合「{status.incomplete.input}」没有完成
              {status.incomplete.happened.length > 0 ? `（已执行：${status.incomplete.happened.join("、")}，结果保留在日志中）` : "（还没有执行任何效果）"}。
              可以重新输入这个行动继续。{" "}
              <button
                className="link"
                onClick={() =>
                  void api
                    .dismissIncompleteTurn(ctx.worldId, campaignId)
                    .then(refresh)
                    .catch((e: Error) => ctx.notify(e.message, "error"))
                }
              >
                知道了
              </button>
            </div>
          )}
          {showPicker && (
            <PcPicker
              ctx={ctx}
              scriptId={status?.campaign.script_id ?? null}
              campaignId={campaignId}
              onDone={() => {
                setPicking(false);
                void api.campaignPc(ctx.worldId, campaignId).then(setPc).catch(() => {});
                void send(OPENING);
              }}
            />
          )}
          {showOpening && (
            <div className="opening-cta card">
              {pc && <p>你扮演：<strong>{pc.name}</strong>（{pc.identity}）</p>}
              <button className="primary" data-tour="opening" onClick={() => void send(OPENING)}>
                听 GM 讲述开场
              </button>
              <p className="hint">也可以直接在下面输入第一个行动。</p>
            </div>
          )}
          {lines.length === 0 && !atStart && <p className="hint">输入玩家的第一个行动，开始游戏。</p>}
          {lines.map((l, i) =>
            l.who === "gm" && !l.gmOnly ? null : l.who === "npc" || l.who === "player" ? (
              <div key={`${l.seq}-${i}`} className={`bubble-row from-${l.who}${initialLines.current && !initialLines.current.has(`${l.seq}-${i}`) ? " enter" : ""}`}>
                <span className="avatar" aria-hidden="true">
                  {l.who === "player" ? (pc?.name ?? "我").slice(0, 1) : (l.name ?? "?").slice(0, 1)}
                </span>
                <div className="bubble">
                  <div className="who">{l.who === "player" ? pc?.name ?? "你" : l.name}</div>
                  {l.text}
                  {l.who === "npc" && (
                    <button className="link speak" title="朗读" onClick={() => voice.enqueue(l.speaker || null, l.text)}>
                      🔊
                    </button>
                  )}
                </div>
              </div>
            ) : (
            // 系统提示、掷骰记录、只给主持人看的内容：小胶囊。
            <div key={`${l.seq}-${i}`} className={`line${initialLines.current && !initialLines.current.has(`${l.seq}-${i}`) ? " enter" : ""} who-${l.who}${l.gmOnly ? " gm-only" : ""}${l.warn ? " warn" : ""}`}>
              {l.text}
              {l.who === "gm" && <MentionThumbs ctx={ctx} text={l.text} nodes={illustrated} onOpen={setGallery} />}
            </div>
            ),
          )}
          {pendingSay && (
            <div className="bubble-row from-player enter pending">
              <span className="avatar" aria-hidden="true">
                {(pc?.name ?? "我").slice(0, 1)}
              </span>
              <div className="bubble">
                <div className="who">{pc?.name ?? "你"}</div>
                {pendingSay}
              </div>
            </div>
          )}
          {interactions.map((ix) => (
            <InteractionHost
              key={ix.interaction_id}
              ctx={ctx}
              campaignId={campaignId}
              interaction={ix}
              onDice={(roll, label) => (lowPower ? setCheckQueue((q) => [...q, { skill: label, reason: "", detail: roll }]) : setTray((q) => [...q, checkToRecord(label, "", roll)]))}
              onChanged={() => {
                setIxKey((k) => k + 1);
                ctx.bump();
              }}
              onFinished={(done) => {
                setInteractions((q) => q.filter((x) => x.interaction_id !== done.interaction_id));
                // 结果交给 GM 叙述（作为玩家这一回合的行动）；放弃的不用叙述。
                if (done.status !== "abandoned") void send(resultLine(done));
              }}
            />
          ))}
          {busy && <div className="line who-system">{diceAsk ? "轮到你掷骰：在右边的玻璃球里掷，或交给程序掷" : gmLocal ? "GM 正在构思，本地模型可能需要一段时间（可以点输入框旁的「取消」）" : "GM 正在构思（可以点输入框旁的「取消」）"}</div>}
          <div ref={bottom} />
        </div>
        {!inBattle && <SmartCards last={last} sceneKey={status?.current?.id ?? ""} onOpenTab={openTab} />}
        {downed && !inBattle && !busy && status?.campaign.status !== "ended" && (
          <div className="downed-bar" role="status">
            <span>
              <strong>{party?.name}倒下了</strong>（生命 0/{party?.max}）：伤得太重站不起来，要先休息恢复，或者向附近的人求助、用药治疗。
            </span>
            <button className="primary" onClick={() => void send("我躺下休息，先把伤养一养")}>
              休息
            </button>
            <button onClick={() => void send("我向附近的人呼救求助")}>向人求助</button>
          </div>
        )}
        {gmChoices.length > 0 && !busy && !inBattle && !downed && status?.campaign.status !== "ended" && (
          <div className="choices" data-tour="choices">
            {gmChoices.map((c, i) => (
              <button key={`${c}-${i}`} type="button" className="choice" style={{ animationDelay: `${i * 0.06}s` }} onClick={() => void send(c)} title="直接作为这一回合的行动">
                {c}
              </button>
            ))}
          </div>
        )}
        {!inBattle && !showPicker && status?.campaign.status !== "ended" && (
          <SceneBar ctx={ctx} campaignId={campaignId} refreshKey={ixKey + ctx.tick} disabled={busy} onOpened={(i) => setInteractions((q) => [...q.filter((x) => x.interaction_id !== i.interaction_id), i])} />
        )}
        {rituals.length > 0 && <RitualLayer worldId={ctx.worldId} queue={rituals} lowPower={lowPower} onDone={(n) => setRituals((q) => q.slice(n))} />}
        <form
          className="row input-row"
          // 选角色时还不能行动：先藏起输入框，免得盖住「开始游戏」按钮。
          style={showPicker ? { display: "none" } : undefined}
          onSubmit={(e) => {
            e.preventDefault();
            void send();
          }}
        >
          <textarea
            data-tour="input"
            rows={2}
            value={input}
            disabled={busy || status?.campaign.status === "ended"}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                void send();
              }
            }}
            placeholder={status?.campaign.status === "ended" ? "这条时间线已经结束。可以在右侧「时间线」中从任意历史节点开出新时间线继续" : "描述玩家的行动或台词（Enter 发送，Shift+Enter 换行）"}
          />
          <MicButton disabled={busy || status?.campaign.status === "ended"} notify={ctx.notify} onText={(t) => setInput((cur) => (cur.trim() ? cur.trimEnd() + " " + t : t))} />
          {busy ? (
            <button type="button" onClick={() => void api.cancelGmTurn(campaignId)}>
              取消
            </button>
          ) : (
            <button type="submit" className="primary" disabled={!input.trim()}>
              行动
            </button>
          )}
        </form>
      </div>

      <aside className="gm-col" aria-label="GM">
        <div
          className="gm-resize"
          role="separator"
          aria-orientation="vertical"
          aria-label="拖动调整 GM 窗口宽度"
          title="拖动调整宽度，双击恢复默认"
          onPointerDown={startGmResize}
          onDoubleClick={() => {
            setGmW(GM_W_DEFAULT);
            try {
              localStorage.removeItem("gm.width");
            } catch {
              /* 忽略 */
            }
          }}
        />
          <div className="gm-stage" data-tour="gm-stage" ref={stageRef}>
            <GmOrb
              state={orbState}
              stage={stage}
              text={speaking ?? ""}
              level={voice.level}
              reveal={voice.progress}
              onSkipWait={stage === "voice" ? voice.skipWait : undefined}
              onFinished={() => {
                setSpeaking(null);
                // 讲完后留在球里，显示完整的这一段（玩家行动时自动收起）。
                setReadIdx(LATEST);
                setUnread(false);
              }}
              check={checkQueue[0] ?? null}
              onCheckDone={() => setCheckQueue((q) => q.slice(1))}
              diceAsk={diceAsk ? { ask: diceAsk, mode: diceMode === "auto" ? "click" : diceMode } : null}
              onDiceAnswer={answerDice}
              shader={!lowPower}
              theme={orbTheme}
              history={gmSaid}
              readIndex={null}
              onRead={() => setUnread(false)}
              compact
              onTyped={setTypedN}
              unread={unread}
              footer={
                readAt != null && gmSaid[readAt] ? (
                  <>
                    <button className="link speak" title="朗读这一段" onClick={(e) => { e.stopPropagation(); voice.enqueue(null, gmSaid[readAt].text); }}>
                      🔊 朗读
                    </button>
                    <MentionThumbs ctx={ctx} text={gmSaid[readAt].text} nodes={illustrated} onOpen={setGallery} />
                  </>
                ) : null
              }
            />
          </div>
        <div className="gm-log" ref={gmLogRef}>
          {gmSaid.length === 0 && !speaking && <p className="hint gm-log-empty">GM 的讲述会出现在这里。</p>}
          {lines.map((l, i) =>
            l.who === "gm" && !l.gmOnly && !(speaking && l.text.trim() === speaking.trim()) ? (
              <div key={`${l.seq}-${i}`} className={`story-card${initialLines.current && !initialLines.current.has(`${l.seq}-${i}`) ? " enter" : ""}`}>
                <div className="story-meta">
                  <span className="dot" />
                  GM · 第 {l.turn + 1} 回合
                  <button className="link speak" title="朗读这一段" onClick={() => voice.enqueue(null, l.text)}>
                    🔊
                  </button>
                </div>
                <div className="story-text">{l.text}</div>
                <MentionThumbs ctx={ctx} text={l.text} nodes={illustrated} onOpen={setGallery} />
              </div>
            ) : null,
          )}
          {speaking && (
            <div className="story-card typing enter">
              <div className="story-meta">
                <span className="dot" />
                GM · 正在讲述
                <button className="link speak" title="跳过，直接显示全文" onClick={() => setSpeaking(null)}>
                  跳过 ⏭
                </button>
              </div>
              <div className="story-text">{speaking.slice(0, Math.max(typedN, 0))}</div>
            </div>
          )}
        </div>
      </aside>

      {(() => {
        const on = immersive ? railOpen : drawer;
        return (
          <button
            type="button"
            className={`panel-fab${on ? " on" : ""}`}
            onClick={() => {
              if (!immersive) return setDrawer((d) => !d);
              if (railOpen) {
                setDrawer(false);
                setSetsOpen(false);
              }
              setRailOpen(!railOpen);
            }}
            title={on ? "收起面板" : "展开游戏面板：状态、任务、角色、存档、设置"}
            data-tour="panel-fab"
          >
            {on ? "✕" : "☰"}
            <span>{on ? "收起" : "面板"}</span>
          </button>
        );
      })()}
      <div className={immersive ? "detail play-drawer" : `detail side-pop${drawer ? " open" : ""}`} aria-hidden={!drawer}>
        {immersive ? (
          <div className="drawer-head">
            <strong>{tabTitle}</strong>
            <button className="link" onClick={() => setDrawer(false)} title="收起（Esc）">
              ✕
            </button>
          </div>
        ) : (
          <div className="row">{settingsControls}</div>
        )}
        <PendingImages ctx={ctx} />
        {!immersive && <RunMonitor refreshKey={events.length} openDefault={false} />}
        {!immersive && <WebSearchPanel notify={ctx.notify} />}
        {!immersive && (
          <div className="tabs" data-tour="tabs">
            {tabList.map(([k, v]) => (
              <button key={k} className={tab === k ? "active" : ""} onClick={() => setTab(k)} data-tour={`tab-${k}`}>
                {v}
              </button>
            ))}
          </div>
        )}
        {tab === "state" && st && (
          <div className={`card state${flash === "state" ? " flash" : ""}`} ref={stateRef}>
            <div>回合 <AnimatedNumber value={status?.campaign.turn ?? 0} /></div>
            <div>
              <strong>任务</strong>：{Object.entries(st.quests).map(([q, s]) => `${names(q)}（${s}）`).join("，") || "—"}
            </div>
            <div>
              <strong>线索</strong>：{st.clues.map(names).join("，") || "—"}
            </div>
            <div>
              <strong>物品</strong>：{Object.entries(st.items).filter(([, n]) => n > 0).map(([k, n]) => `${names(k.split("|")[1] ?? k)}×${n}`).join("，") || "—"}
            </div>
            <div>
              <strong>NPC 态度</strong>：
              {Object.entries(st.vars)
                .filter(([k]) => k.startsWith("attitude."))
                .map(([k, v]) => `${names(k.slice(9))} ${String(v)}`)
                .join("，") || "—"}
            </div>
            {gmView && (
              <div>
                <strong>标志与变量</strong>：{Object.entries(st.vars).filter(([k]) => !k.startsWith("attitude.")).map(([k, v]) => `${k}=${JSON.stringify(v)}`).join("，") || "—"}
              </div>
            )}
            {last && last.checks.length > 0 && (
              <div className="last-checks">
                <strong>上一回合的检定</strong>
                {last.checks.map((c, i) => (
                  <div key={i} className="check-line">
                    🎲 <b>{c.skill}</b>
                    {c.reason ? `（${c.reason}）` : ""}：{c.detail?.explain ?? `${c.roll}+${c.modifier}=${c.total} / 难度 ${c.dc} → ${c.success ? "成功" : "失败"}`}
                  </div>
                ))}
              </div>
            )}
            {gmView && last && (
              <div className="last-turn">
                <strong>上一回合（程序执行）</strong>
                <div>意图：{last.intent || "—"}{last.off_script && "（剧本外行动）"}</div>
                {last.applied.length > 0 && <div>已执行 {last.applied.length} 条状态修改</div>}
                {last.rejected.map((r, i) => (
                  <div key={i} className="err">
                    拒绝：{r.reason}
                  </div>
                ))}
                {last.transition_note && <div className="err">{last.transition_note}</div>}
                {last.entities.map((e, i) => (
                  <div key={i}>
                    新内容「{e.name}」：{e.note}
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
        {tab === "branches" && status && (
          <div className="card">
            <p className="hint">程序判定的当前可选分支（GM 只能从中选择推进剧情）。</p>
            {status.options.length === 0 && <p className="hint">当前没有预设分支，GM 会自由主持。</p>}
            <ul className="options">
              {status.options.map((o) => (
                <li key={o.edge.id} className={`opt tri-${o.availability}`}>
                  <div>
                    <strong>{o.edge.label}</strong> <span className="badge">{o.used_up ? "已走过" : TRI[o.availability]}</span>
                    {gmView && <div className="hint">{describeCondition(o.edge.condition, names)}</div>}
                  </div>
                </li>
              ))}
            </ul>
          </div>
        )}
        {tab === "memory" && memory && (
          <div className="card">
            <div className="row">
              <button onClick={() => void summarize()}>生成本次会话摘要</button>
            </div>
            <MemoryCompression ctx={ctx} campaignId={campaignId} />
            {memory.summaries.map((s, i) =>
              s.status === "auto" ? (
                <AutoSummary key={s.id} ctx={ctx} campaignId={campaignId} s={s} latest={!memory.summaries.slice(i + 1).some((x) => x.status === "auto" || x.status === "confirmed")} />
              ) : s.status === "draft" ? (
                <div key={s.id} className="form">
                  <h3>摘要草稿（确认后 GM 才会参考）</h3>
                  <textarea rows={6} value={draft} onChange={(e) => setDraft(e.target.value)} />
                  <button className="primary" onClick={() => void confirmDraft(s.id)}>
                    确认
                  </button>
                </div>
              ) : (
                <div key={s.id}>
                  <strong>第 {s.session_no} 次会话</strong>
                  <p>{s.summary}</p>
                </div>
              ),
            )}
            {gmView && memory.facts.length > 0 && (
              <>
                <h3>GM 记住的事实</h3>
                <ul>
                  {memory.facts.map((f, i) => (
                    <li key={i}>
                      {f}{" "}
                      <button
                        className="link"
                        title="GM 以后不再记得这件事（原始记录保留）"
                        onClick={() =>
                          void api
                            .forgetFact(ctx.worldId, campaignId, memory.fact_seqs[i])
                            .then(ctx.bump)
                            .catch((e: Error) => ctx.notify(e.message, "error"))
                        }
                      >
                        忘掉
                      </button>
                    </li>
                  ))}
                </ul>
              </>
            )}
            {memory.entities.length > 0 && (
              <>
                <h3>本战役中出现的新内容</h3>
                <ul>
                  {memory.entities.map((e) => (
                    <li key={e.id}>
                      [{e.kind}] {e.name}：{e.summary}
                      {gmView && e.gm_text && <span className="gm">（{e.gm_text}）</span>}
                      {e.changeset_id && (
                        <button className="link" onClick={() => ctx.go({ name: "review", id: e.changeset_id! })}>
                          审阅写入世界
                        </button>
                      )}
                    </li>
                  ))}
                </ul>
              </>
            )}
          </div>
        )}
        {tab === "board" && <InvestigationPanel ctx={ctx} campaignId={campaignId} gmView={gmView} ended={status?.campaign.status === "ended"} refreshKey={ctx.tick} />}
        {tab === "quests" && <QuestPanel ctx={ctx} campaignId={campaignId} gmView={gmView} names={names} refreshKey={ctx.tick} />}
        {tab === "npcs" && <NpcPanel ctx={ctx} campaignId={campaignId} gmView={gmView} ended={status?.campaign.status === "ended"} refreshKey={ctx.tick} onChanged={() => { void refresh(); ctx.bump(); }} />}
        {tab === "console" && gmView && <ConsolePanel ctx={ctx} campaignId={campaignId} ended={status?.campaign.status === "ended"} refreshKey={ctx.tick} choices={choices} onChanged={() => { void refresh(); ctx.bump(); }} />}
        {tab === "character" && <CharacterPanel ctx={ctx} campaignId={campaignId} gmView={gmView} ended={status?.campaign.status === "ended"} refreshKey={ctx.tick} onChanged={() => { void refresh(); ctx.bump(); }} />}
        {tab === "combat" && <CombatPanel ctx={ctx} campaignId={campaignId} gmView={gmView} ended={status?.campaign.status === "ended"} refreshKey={ctx.tick} onChanged={() => { void refresh(); ctx.bump(); }} />}
        {tab === "world" && <WorldPanel ctx={ctx} campaignId={campaignId} gmView={gmView} ended={status?.campaign.status === "ended"} refreshKey={ctx.tick} onChanged={() => { void refresh(); ctx.bump(); }} />}
        {tab === "relations" && (
          <div className="card relations-embed">
            <Suspense fallback={<p className="hint">正在载入关系图谱…</p>}>
              <NetworkEmbed ctx={ctx} lockedCampaign={campaignId} />
            </Suspense>
          </div>
        )}
        {tab === "timeline" && status && <TimelinePanel ctx={ctx} campaign={status.campaign} campaigns={campaigns} onSwitch={onSwitch} onChanged={() => { void refresh(); ctx.bump(); }} />}
      </div>
      {gallery && <NodeGallery ctx={ctx} target={gallery} gmView={gmView} onClose={() => setGallery(null)} />}
    </div>
  );
}

/** GM 回合里的检定 → 骰子托盘的记录（优势 / 劣势的另一次掷骰也画出来，但不计入）。 */
export function checkToRecord(skill: string, reason: string, d: ShownCheck["detail"]): DiceRecord {
  const m = /d(\d+)/i.exec(d.expr);
  const sides = m ? Number(m[1]) : 20;
  const all = [...d.dice, ...d.other];
  return {
    purpose: `${skill} 检定${reason ? `：${reason}` : ""}`,
    category: "check",
    expr: d.mode === "pool" ? d.expr : d.advantage ? `${d.expr}（${d.advantage > 0 ? "优势" : "劣势"}）` : d.expr,
    dice: all,
    sides: all.map(() => sides),
    kept: d.dice.map((_, i) => i),
    natural: d.natural,
    parts: d.mode === "target" && d.modifier ? [["修正", d.modifier]] : [],
    total: d.total,
    target: d.mode === "roll_under" ? d.target : d.dc,
    outcome: DEGREE_TEXT[d.degree] ?? "",
    stream: 0,
    manual: d.explain.includes("亲手"),
  };
}
const DEGREE_TEXT: Record<string, string> = { crit_success: "大成功", success: "成功", partial: "部分成功", failure: "失败", crit_failure: "大失败" };

const CP_LABEL: Record<string, string> = { start: "战役开始", turn_end: "回合结束", transition: "剧情推进", ending: "战役结束", fork: "时间线起点", migrated: "升级时补建", init: "世界初始化完成" };

/** 结束战役、从历史节点开出新时间线、校验、重命名、归档、比较结局。 */
function TimelinePanel({ ctx, campaign, campaigns, onSwitch, onChanged }: { ctx: Ctx; campaign: Campaign; campaigns: Campaign[]; onSwitch: (id: string) => void; onChanged: () => void }) {
  const [ending, setEnding] = useState("");
  const [points, setPoints] = useState<ForkPoint[]>([]);
  const [reseed, setReseed] = useState(false);
  const [verify, setVerify] = useState<VerifyReport | null>(null);
  const [name, setName] = useState(campaign.name);
  const [picked, setPicked] = useState<string[]>([campaign.id]);
  const [cmp, setCmp] = useState<TimelineComparison | null>(null);
  const [onlyDiff, setOnlyDiff] = useState(true);
  const ended = campaign.status === "ended";
  const parent = campaigns.find((c) => c.id === campaign.parent_id);
  const family = useMemo(() => {
    const root = timelineTree(campaigns, true).find((r) => r.campaign.id === campaign.id)?.root;
    return timelineTree(campaigns, true).filter((r) => r.root === root);
  }, [campaigns, campaign.id]);

  // 存档点（进行中的战役也可以读档：从存档点开出新时间线）；结束后显示全部历史节点。
  const [saveName, setSaveName] = useState("");
  const loadPoints = () => void api.timelineForkPoints(ctx.worldId, campaign.id).then(setPoints).catch((e: Error) => ctx.notify(e.message, "error"));
  useEffect(loadPoints, [ctx.worldId, campaign.id, ended, ctx.tick]);
  const saves = points.filter((p) => p.checkpoint.label.startsWith("save:"));
  const loadFrom = (p: ForkPoint, label: string) => {
    const n = prompt("读档会从这里开出一条新时间线（现在这条保持不变）。新时间线的名称：", `${campaign.name}（${label}）`);
    if (n === null) return;
    void run(async () => {
      const c = await api.timelineFork(ctx.worldId, campaign.id, p.checkpoint.id, n, false);
      ctx.notify(`已读档：新时间线「${c.name}」`);
      onSwitch(c.id);
    });
  };

  const run = async (f: () => Promise<unknown>) => {
    try {
      await f();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  };

  return (
    <div className="card">
      {parent && (
        <p className="hint">
          从「{parent.name}」第 {campaign.fork_turn} 回合分出的时间线。{" "}
          <button className="link" onClick={() => onSwitch(parent.id)}>查看原时间线</button>
        </p>
      )}
      {!ended && (
        <div className="saves">
          <h3>存档</h3>
          <div className="row wrap">
            <input value={saveName} onChange={(e) => setSaveName(e.target.value)} placeholder="存档名（例如：进潮洞之前）" style={{ flex: 1, minWidth: 160 }} />
            <button
              onClick={() =>
                void run(async () => {
                  await api.savePoint(ctx.worldId, campaign.id, saveName || "存档 " + new Date().toLocaleString());
                  setSaveName("");
                  ctx.notify("已存档");
                  loadPoints();
                })
              }
            >
              存档
            </button>
            <button
              title="不用起名，随时覆盖式地记一下"
              onClick={() =>
                void run(async () => {
                  await api.savePoint(ctx.worldId, campaign.id, "");
                  ctx.notify("已快速存档");
                  loadPoints();
                })
              }
            >
              快速存档
            </button>
            <button
              title="从最近一次快速存档开出新时间线（现在这条保持不变）"
              onClick={() =>
                confirm("从最近一次快速存档读档？会开出一条新时间线，现在这条保持不变。") &&
                void run(async () => {
                  const c = await api.quickLoad(ctx.worldId, campaign.id);
                  ctx.notify(`已读档：「${c.name}」`);
                  onSwitch(c.id);
                })
              }
            >
              快速读档
            </button>
          </div>
          {saves.length > 0 && (
            <ul className="list">
              {[...saves].reverse().map((p) => (
                <li key={p.checkpoint.id} className="row">
                  <span style={{ flex: 1 }}>
                    <strong>{p.checkpoint.label.slice(5)}</strong> <small className="hint">第 {p.checkpoint.turn} 回合 · {new Date(p.checkpoint.created_at).toLocaleString()}</small>
                    {p.last_narration && <div className="hint">{p.last_narration}</div>}
                  </span>
                  <button className="small" onClick={() => loadFrom(p, p.checkpoint.label.slice(5))}>
                    读档
                  </button>
                </li>
              ))}
            </ul>
          )}
          <h4>回到任意历史节点</h4>
          <p className="hint">每个回合结束都会自动保存检查点。读档会开出新时间线，现在这条保持不变。</p>
          <details>
            <summary>显示全部 {points.length} 个检查点</summary>
            <ul className="list fork-points">
              {points.map((p) => (
                <li key={p.checkpoint.id} className="row">
                  <span style={{ flex: 1 }}>
                    <span className="kind">第 {p.checkpoint.turn} 回合 · {p.checkpoint.label.startsWith("save:") ? p.checkpoint.label.slice(5) : CP_LABEL[p.checkpoint.label] ?? p.checkpoint.label}</span>
                    {p.last_action && <span>玩家：{p.last_action}</span>}
                  </span>
                  <button className="small" onClick={() => loadFrom(p, `回到第 ${p.checkpoint.turn} 回合`)}>
                    从这里开出新时间线
                  </button>
                </li>
              ))}
            </ul>
          </details>
        </div>
      )}
      {!ended && (
        <>
          <h3>结束这条时间线</h3>
          <p className="hint">战役结束后会保存完整快照。之后可以回到任意一个历史节点，开出新的时间线继续玩，原来的时间线保持不变。结束后这条时间线不能再继续推进。</p>
          <div className="row">
            <input value={ending} onChange={(e) => setEnding(e.target.value)} placeholder="结局说明（可空，默认取当前剧情节点名）" style={{ flex: 1 }} />
            <button
              onClick={() =>
                confirm("结束这条时间线？结束后不能继续推进，只能查看或回溯。") &&
                void run(async () => {
                  await api.endCampaign(ctx.worldId, campaign.id, ending);
                  ctx.notify("已结束，并保存了完整快照");
                  onChanged();
                })
              }
            >
              结束战役
            </button>
          </div>
        </>
      )}
      {ended && (
        <>
          <h3>已结束：{campaign.ending || "—"}</h3>
          <h4>回到历史节点，开出新时间线</h4>
          <label className="check">
            <input type="checkbox" checked={reseed} onChange={(e) => setReseed(e.target.checked)} />
            新时间线换新的骰子种子（不勾选时，之后的掷骰与原时间线当时完全相同）
          </label>
          <ul className="list fork-points">
            {points.map((p) => (
              <li key={p.checkpoint.id} className="row">
                <span style={{ flex: 1 }}>
                  <span className="kind">第 {p.checkpoint.turn} 回合 · {p.checkpoint.label.startsWith("save:") ? p.checkpoint.label.slice(5) : CP_LABEL[p.checkpoint.label] ?? p.checkpoint.label}</span>
                  {p.last_action && <span>玩家：{p.last_action}</span>}
                  {p.last_narration && <div className="hint">{p.last_narration}</div>}
                </span>
                {p.checkpoint.label !== "ending" && (
                  <button
                    onClick={() => {
                      const n = prompt("新时间线的名称", `${campaign.name}（回到第 ${p.checkpoint.turn} 回合）`);
                      if (n === null) return;
                      void run(async () => {
                        const c = await api.timelineFork(ctx.worldId, campaign.id, p.checkpoint.id, n, reseed);
                        ctx.notify(`已开出新时间线「${c.name}」，原时间线保持不变`);
                        onSwitch(c.id);
                      });
                    }}
                  >
                    从这里开出新时间线
                  </button>
                )}
              </li>
            ))}
          </ul>
        </>
      )}

      <h4>校验</h4>
      <button onClick={() => void run(async () => setVerify(await api.timelineVerify(ctx.worldId, campaign.id)))}>检查回溯数据完整性</button>
      {verify && (
        <div className={verify.ok ? "ok-text" : "warn-text"}>
          {verify.ok ? `通过：重放 ${verify.events} 条事件，与 ${verify.checkpoints} 个检查点完全一致。` : "发现问题："}
          {verify.checksum_failures.length > 0 && <div>校验值不符的检查点：{verify.checksum_failures.join("、")}</div>}
          {verify.mismatches.map((m, i) => <div key={i}>{m}</div>)}
          {verify.notes.map((m, i) => <div key={i} className="hint">{m}</div>)}
        </div>
      )}

      <h4>名称与归档</h4>
      <div className="row">
        <input value={name} onChange={(e) => setName(e.target.value)} style={{ flex: 1 }} />
        <button onClick={() => void run(async () => { await api.timelineRename(ctx.worldId, campaign.id, name); onChanged(); })}>重命名</button>
        <button onClick={() => void run(async () => { await api.timelineArchive(ctx.worldId, campaign.id, !campaign.archived); onChanged(); })}>
          {campaign.archived ? "取消归档" : "归档"}
        </button>
      </div>

      {family.length > 1 && (
        <>
          <h4>比较时间线</h4>
          {family.map((r) => (
            <label key={r.campaign.id} className="check">
              <input
                type="checkbox"
                checked={picked.includes(r.campaign.id)}
                onChange={(e) => setPicked((p) => (e.target.checked ? [...p, r.campaign.id] : p.filter((x) => x !== r.campaign.id)))}
              />
              {timelineLabel(r)}
            </label>
          ))}
          <div className="row">
            <button disabled={picked.length < 2} onClick={() => void run(async () => setCmp(await api.timelineCompare(ctx.worldId, picked)))}>比较</button>
            <label className="check">
              <input type="checkbox" checked={onlyDiff} onChange={(e) => setOnlyDiff(e.target.checked)} />
              只看不同之处
            </label>
          </div>
          {cmp && (
            <table className="data">
              <thead>
                <tr>
                  <th />
                  {cmp.campaigns.map((c) => <th key={c.id}>{c.name}</th>)}
                </tr>
              </thead>
              <tbody>
                {cmp.rows.filter((r) => !onlyDiff || r.differs || r.category === "结局").map((r, i) => (
                  <tr key={i} className={r.differs ? "differs" : ""}>
                    <td><small className="hint">{r.category}</small> {r.key}</td>
                    {r.values.map((v, j) => <td key={j}>{v ?? "—"}</td>)}
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      )}
    </div>
  );
}

/** 朗读队列：按顺序合成并播放；停止时清空。合成在后端进行（有缓存、去重和队列上限）。 */
const SKIPPED = "__skip_voice__";

function useVoice(ctx: Ctx) {
  const queue = useRef<{ speaker: string | null; text: string }[]>([]);
  const audio = useRef<HTMLAudioElement | null>(null);
  // 实时音量（WebAudio 分析器）：GM 玻璃球讲述时随声音起伏。
  const ac = useRef<{ ctx: AudioContext; an: AnalyserNode; buf: Uint8Array<ArrayBuffer> } | null>(null);
  const level = useCallback((): number | null => {
    const a = audio.current;
    const g = ac.current;
    if (!a || a.paused || !g) return null;
    g.an.getByteTimeDomainData(g.buf);
    let sum = 0;
    for (const v of g.buf) sum += ((v - 128) / 128) ** 2;
    return Math.min(1, Math.sqrt(sum / g.buf.length) * 4);
  }, []);
  const running = useRef(false);
  /** 每次停止都加一：旧的播放循环看到编号变了就退出。 */
  const gen = useRef(0);
  const [playing, setPlaying] = useState(false);

  /** 播放一段音频，播完（或出错）时返回。出错时抛出原因，不再悄悄跳过（以前播放失败没有任何提示）。 */
  const playClip = useCallback((url: string) => {
    return new Promise<void>((resolve, reject) => {
      const a = new Audio(url);
      audio.current = a;
      try {
        if (!ac.current) {
          const c = new AudioContext();
          const an = c.createAnalyser();
          an.fftSize = 1024;
          an.connect(c.destination);
          ac.current = { ctx: c, an, buf: new Uint8Array(an.fftSize) };
        }
        // 音频上下文没有在运行时不接分析器：接上会把声音送进一个不出声的上下文。
        if (ac.current.ctx.state === "running") ac.current.ctx.createMediaElementSource(a).connect(ac.current.an);
        else void ac.current.ctx.resume();
      } catch {
        /* 音量分析不可用时只影响玻璃球的起伏，不影响朗读 */
      }
      a.onended = () => resolve();
      a.onerror = () => reject(new Error(`语音播放失败：${a.error?.message || "音频无法加载"}`));
      void a.play().catch((e: Error) => reject(new Error(`语音播放失败：${e.message}`)));
    });
  }, []);

  const pump = useCallback(async () => {
    if (running.current) return;
    running.current = true;
    const my = gen.current;
    setPlaying(true);
    try {
      while (queue.current.length && gen.current === my) {
        const item = queue.current.shift()!;
        try {
          const r = await api.voiceSpeak(ctx.worldId, item.speaker, item.text);
          if (gen.current !== my) break;
          await playClip(r.data_url);
        } catch (e) {
          ctx.notify((e as Error).message, "error");
          queue.current = [];
        }
      }
    } finally {
      if (gen.current === my) {
        running.current = false;
        audio.current = null;
        setPlaying(false);
      }
    }
  }, [ctx, playClip]);
  const enqueue = useCallback(
    (speaker: string | null, text: string) => {
      if (!text.trim()) return;
      queue.current.push({ speaker, text });
      void pump();
    },
    [pump],
  );
  const stop = useCallback(() => {
    gen.current++;
    queue.current = [];
    running.current = false;
    sync.current = null;
    audio.current?.pause();
    audio.current = null;
    setPlaying(false);
  }, []);

  // 先合成、再输出：讲述切成几段，第一段合成好才开始（文字和声音一起出现），后面的段在播放时依次合成。
  // progress() 返回声音已经读到全文的几分之几，玻璃球按它显示文字。
  const sync = useRef<{ lens: number[]; total: number; done: number; cur: number } | null>(null);
  const abortWait = useRef<(() => void) | null>(null);
  /** 不等语音合成完，先显示文字（这一段不朗读）。 */
  const skipWait = useCallback(() => {
    const f = abortWait.current;
    stop();
    f?.();
  }, [stop]);
  const progress = useCallback((): number | null => {
    const st = sync.current;
    if (!st) return null;
    const a = audio.current;
    let chars = st.done;
    if (a && Number.isFinite(a.duration) && a.duration > 0) chars += st.lens[st.cur] * Math.min(1, a.currentTime / a.duration);
    return Math.min(1, chars / st.total);
  }, []);
  /** 返回第一段是否合成好（false = 失败，调用方按没有声音的方式显示文字）。播放在后台继续，播完再接着读队列里的台词。 */
  const speakSynced = useCallback(
    async (speaker: string | null, text: string): Promise<boolean> => {
      stop();
      const my = gen.current;
      const chunks = splitForSpeech(text);
      if (!chunks.length) return false;
      running.current = true;
      setPlaying(true);
      // 依次合成（本机语音服务一次只能合成一段，排队即可）。
      const results: Promise<string>[] = [];
      let chain: Promise<unknown> = Promise.resolve();
      for (const c of chunks) {
        const pr = chain.then(() => (gen.current === my ? api.voiceSpeak(ctx.worldId, speaker, c).then((r) => r.data_url) : Promise.reject(new Error("已停止"))));
        results.push(pr);
        chain = pr.catch(() => {});
      }
      const finish = () => {
        if (gen.current !== my) return;
        sync.current = null;
        running.current = false;
        audio.current = null;
        setPlaying(false);
        if (queue.current.length) void pump();
      };
      let first: string;
      try {
        // 「不等语音，先看文字」会让这里提前结束。
        first = await Promise.race([results[0], new Promise<string>((_, rej) => (abortWait.current = () => rej(new Error(SKIPPED))))]);
      } catch (e) {
        if (gen.current === my && (e as Error).message !== SKIPPED) ctx.notify((e as Error).message, "error");
        finish();
        return false;
      } finally {
        abortWait.current = null;
      }
      if (gen.current !== my) return false;
      const lens = chunks.map((c) => c.length);
      sync.current = { lens, total: lens.reduce((a, b) => a + b, 0), done: 0, cur: 0 };
      void (async () => {
        try {
          for (let i = 0; i < chunks.length && gen.current === my; i++) {
            const url = i === 0 ? first : await results[i];
            if (gen.current !== my) return;
            sync.current!.cur = i;
            await playClip(url);
            // 先清掉已经播完的音频再记账，免得这一段被算两次（文字会突然往前跳一段）。
            audio.current = null;
            if (sync.current) sync.current.done += lens[i];
          }
        } catch (e) {
          if (gen.current === my) ctx.notify((e as Error).message, "error");
        } finally {
          finish();
        }
      })();
      return true;
    },
    [ctx, pump, playClip, stop],
  );
  return { enqueue, stop, playing, level, speakSynced, progress, skipWait };
}
