// 大厅（打开软件的默认页）：左边剧本库与最近游戏，中间大玻璃球，右边创建游戏 / 继续游戏 / 游戏设置。
// 创建游戏：起名 → 选角色 → 确认设置 → 世界初始化（7 步真实后台任务，进度来自 game-state 事件）
// → 玻璃球平移到游戏界面顶部，自动讲开场白。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import type { Ctx } from "../App";
import type { Campaign } from "../bindings/Campaign";
import type { Script } from "../bindings/Script";
import type { WorldInfo } from "../bindings/WorldInfo";
import type { GameState } from "../bindings/GameState";
import type { InitStep } from "../bindings/InitStep";
import OrbCanvas, { DEFAULT_THEME, themeFor } from "../components/OrbCanvas";
import PcPicker from "../components/PcPicker";
import { setLowPower, useLowPower } from "../lib/lowPower";

type Panel = "create" | "continue" | "settings";
type Flow = { step: "name" } | { step: "pc"; campaign: Campaign } | { step: "confirm"; campaign: Campaign } | { step: "init"; campaign: Campaign };

const STEP_ICON: Record<InitStep["status"], string> = { pending: "○", running: "◌", done: "●", failed: "✕", skipped: "–" };

export default function Lobby({
  ctx,
  worlds,
  onSelectWorld,
  onEnterGame,
  onOpenStudio,
  onOpenMultiplayer,
  notify,
}: {
  ctx: Ctx | null;
  worlds: WorldInfo[];
  onSelectWorld: (id: string) => void;
  /** 进入游戏空间；`from` 是大厅玻璃球的位置（用来做平移动画）。 */
  onEnterGame: (campaignId: string, from: DOMRect | null, opening: boolean) => void;
  onOpenStudio: (page?: "settings" | "scripts" | "tree" | "general") => void;
  /** 进入联机（开房 / 加入房间）。 */
  onOpenMultiplayer?: () => void;
  notify: (msg: string, kind?: "info" | "error") => void;
}) {
  const lowPower = useLowPower();
  const [scripts, setScripts] = useState<Script[]>([]);
  const [campaigns, setCampaigns] = useState<Campaign[]>([]);
  const [picked, setPicked] = useState<string | null | undefined>(undefined); // undefined = 没选；null = 自由沙盒
  const [panel, setPanel] = useState<Panel>("create");
  const [flow, setFlow] = useState<Flow>({ step: "name" });
  const [name, setName] = useState("");
  const [state, setState] = useState<GameState | null>(null);
  const [orbSize, setOrbSize] = useState(360);
  const orbBox = useRef<HTMLDivElement>(null);
  const worldId = ctx?.worldId ?? null;

  const refresh = useCallback(() => {
    if (!worldId) return;
    api.listScripts(worldId).then(setScripts).catch((e: Error) => notify(e.message, "error"));
    api.listCampaigns(worldId).then((cs) => setCampaigns(cs.filter((c) => !c.archived).sort((a, b) => b.updated_at - a.updated_at))).catch(() => {});
  }, [worldId, notify]);
  useEffect(refresh, [refresh, ctx?.tick]);
  // 删除一局游戏：移到回收站（可以在创作模式的档案馆 / 回收站里恢复）。
  const removeCampaign = async (c: Campaign) => {
    if (!worldId || !window.confirm(`删除游戏「${c.name}」？
会移到回收站，从它分出的时间线一起移走；可以在回收站恢复。`)) return;
    try {
      await api.trashCampaign(worldId, c.id);
      notify(`「${c.name}」已移到回收站`);
      refresh();
    } catch (e) {
      notify((e as Error).message, "error");
    }
  };

  // 玻璃球跟着中间栏大小走。
  useEffect(() => {
    const el = orbBox.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setOrbSize(Math.max(200, Math.min(520, Math.min(el.clientWidth - 40, el.clientHeight - 120)))));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const script = scripts.find((s) => s.id === picked) ?? null;
  const theme = useMemo(() => (script ? themeFor(script.genre, script.title) : picked === null ? themeFor("沙盒", "自由") : DEFAULT_THEME), [script, picked]);
  const scriptName = (id: string | null) => (id ? scripts.find((s) => s.id === id)?.title ?? "剧本" : "自由沙盒");

  // 初始化进度：后端每一步变化都推送 game-state。
  const initCampaign = flow.step === "init" ? flow.campaign.id : null;
  useEffect(() => {
    if (!initCampaign) return;
    let off: (() => void) | undefined;
    let gone = false;
    void listen<{ campaign_id: string; state: GameState }>("game-state", (e) => {
      if (e.payload.campaign_id === initCampaign) setState(e.payload.state);
    }).then((f) => (gone ? f() : (off = f)));
    return () => {
      gone = true;
      off?.();
    };
  }, [initCampaign]);

  async function createCampaign() {
    if (!worldId || picked === undefined) return;
    try {
      const c = await api.createCampaign(worldId, picked, name.trim() || `${scriptName(picked)} · ${new Date().toLocaleDateString()}`);
      setFlow(picked ? { step: "pc", campaign: c } : { step: "confirm", campaign: c });
      refresh();
    } catch (e) {
      notify((e as Error).message, "error");
    }
  }

  async function startInit(c: Campaign) {
    if (!worldId) return;
    setFlow({ step: "init", campaign: c });
    setState(null);
    try {
      const s = await api.gameInit(worldId, c.id);
      setState(s);
      if (s.phase === "opening") {
        // 让最后一步的完成停留一下，再平移进游戏。
        window.setTimeout(() => onEnterGame(c.id, orbBox.current?.querySelector(".orb-canvas, .orb-static")?.getBoundingClientRect() ?? null, true), 650);
      }
    } catch (e) {
      notify((e as Error).message, "error");
    }
  }

  const steps = state?.init ?? [];
  const done = steps.filter((s) => s.status === "done" || s.status === "skipped").length;
  const failed = steps.find((s) => s.status === "failed");
  const initRunning = flow.step === "init" && !failed && state?.phase !== "opening";

  if (!ctx) {
    return (
      <div className="lobby lobby-noworld">
        <div className="lobby-center">
          <OrbCanvas size={300} lowPower={lowPower} />
          <h2>Chronicle GM</h2>
          <p className="hint">先选一个世界。</p>
          <div className="lobby-worlds">
            {worlds.map((w) => (
              <button key={w.id} className="lobby-world" onClick={() => onSelectWorld(w.id)}>
                {w.name}
              </button>
            ))}
            <button
              className="primary"
              onClick={() =>
                void api
                  .openDemoWorld()
                  .then((w) => onSelectWorld(w.id))
                  .catch((e: Error) => notify(e.message, "error"))
              }
            >
              打开示例世界
            </button>
            <button onClick={() => onOpenStudio()}>进入创作模式</button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className={`lobby${flow.step === "init" ? " initializing" : ""}`}>
      <aside className="lobby-left">
        <h3>剧本库</h3>
        <div className="lobby-scripts" data-tour="lobby-scripts">
          <button className={`lobby-script${picked === null ? " on" : ""}`} onClick={() => setPicked(null)}>
            <span className="swatch sandbox" />
            <span>
              <strong>自由沙盒</strong>
              <small>不用剧本，在世界里自由冒险</small>
            </span>
          </button>
          {scripts.map((s) => {
            const t = themeFor(s.genre, s.title);
            return (
              <button key={s.id} className={`lobby-script${picked === s.id ? " on" : ""}`} onClick={() => setPicked(s.id)} title={s.premise}>
                <span className="swatch" style={{ background: `linear-gradient(135deg, rgb(${t.a.map((x) => x * 255).join(",")}), rgb(${t.b.map((x) => x * 255).join(",")}))` }} />
                <span>
                  <strong>{s.title}</strong>
                  <small>
                    {s.genre || "未分类"}
                    {s.duration_hours ? ` · 约 ${s.duration_hours} 小时` : ""}
                  </small>
                </span>
              </button>
            );
          })}
          {scripts.length === 0 && <p className="hint">这个世界还没有剧本，可以先用自由沙盒，或到创作模式里生成剧本。</p>}
        </div>
        <h3>最近游戏</h3>
        <div className="lobby-recent">
          {campaigns.slice(0, 6).map((c, i) => (
            <div key={c.id} className="lobby-recent-row" style={{ animationDelay: `${i * 0.04}s` }}>
              <button className="lobby-recent-item" onClick={() => onEnterGame(c.id, null, false)}>
                <strong>{c.name}</strong>
                <small>
                  {scriptName(c.script_id)} · 第 {c.turn} 回合{c.status === "ended" ? " · 已结束" : ""}
                </small>
              </button>
              <button className="lobby-del" title="删除这局游戏（移到回收站）" aria-label={`删除「${c.name}」`} onClick={() => void removeCampaign(c)}>
                🗑
              </button>
            </div>
          ))}
          {campaigns.length === 0 && <p className="hint">还没有玩过。</p>}
        </div>
      </aside>

      <section className="lobby-center" ref={orbBox} style={{ "--lobby-dot": `rgba(${theme.a.map((x) => Math.round(x * 255)).join(",")},0.45)` } as React.CSSProperties}>
        <OrbCanvas
          theme={theme}
          size={orbSize}
          lowPower={lowPower}
          active={initRunning}
          title={script ? `《${script.title}》` : "GM"}
          onClick={() => {
            if (flow.step === "name" && picked !== undefined) setPanel("create");
          }}
        />
        <div className="lobby-caption" key={picked ?? "none"}>
          {flow.step === "init" ? (
            <>
              <strong>{failed ? "初始化停在了这一步" : state?.phase === "opening" ? "世界已经准备好" : "正在初始化世界"}</strong>
              <div className="init-bar" aria-label={`已完成 ${done} / 7 步`}>
                <span style={{ width: `${(done / 7) * 100}%` }} />
              </div>
            </>
          ) : script ? (
            <>
              <strong>《{script.title}》</strong>
              <p>{script.premise}</p>
            </>
          ) : picked === null ? (
            <>
              <strong>自由沙盒</strong>
              <p>没有预设剧情，GM 跟着你的行动展开世界。</p>
            </>
          ) : (
            <p className="hint">从左边选一个剧本，或继续之前的游戏。</p>
          )}
        </div>
      </section>

      <aside className="lobby-right">
        {flow.step === "init" ? (
          <div className="lobby-card init-steps">
            <h3>世界初始化</h3>
            <ol>
              {(steps.length ? steps : []).map((s) => (
                <li key={s.key} className={`st-${s.status}`}>
                  <span className="ico">{STEP_ICON[s.status]}</span>
                  <span className="lbl">
                    {s.label}
                    {s.ms > 0 && <small> {s.ms < 1000 ? `${s.ms} ms` : `${(s.ms / 1000).toFixed(1)} 秒`}</small>}
                    {s.note && s.status !== "running" && <em>{s.note}</em>}
                  </span>
                </li>
              ))}
              {steps.length === 0 && <li className="st-running">正在启动初始化…</li>}
            </ol>
            {failed && (
              <div className="row">
                <button className="primary" onClick={() => void startInit(flow.campaign)}>
                  重试
                </button>
                {failed.key === "player" && <button onClick={() => setFlow({ step: "pc", campaign: flow.campaign })}>去选角色</button>}
                <button onClick={() => setFlow({ step: "name" })}>返回</button>
              </div>
            )}
            {initRunning && (
              <button className="link" onClick={() => void api.gameInitCancel(flow.campaign.id)}>
                取消
              </button>
            )}
          </div>
        ) : (
          <>
            <div className="lobby-tabs" role="tablist">
              {(
                [
                  ["create", "创建游戏"],
                  ["continue", "继续游戏"],
                  ["settings", "游戏设置"],
                ] as const
              ).map(([k, v]) => (
                <button key={k} role="tab" aria-selected={panel === k} className={panel === k ? "on" : ""} onClick={() => setPanel(k)}>
                  {v}
                </button>
              ))}
            </div>
            {panel === "create" && (
              <div className="lobby-card">
                {flow.step === "name" && (
                  <>
                    <p className="hint">{picked === undefined ? "先在左边选一个剧本或自由沙盒。" : `用「${scriptName(picked)}」开一局新游戏。`}</p>
                    <input placeholder="给这局游戏起个名字（可以不填）" value={name} onChange={(e) => setName(e.target.value)} disabled={picked === undefined} />
                    <button className="primary" disabled={picked === undefined} onClick={() => void createCampaign()} data-tour="lobby-create">
                      下一步：选角色
                    </button>
                  </>
                )}
                {flow.step === "pc" && (
                  <PcPicker
                    ctx={ctx}
                    scriptId={flow.campaign.script_id}
                    campaignId={flow.campaign.id}
                    onDone={() => setFlow({ step: "confirm", campaign: flow.campaign })}
                  />
                )}
                {flow.step === "confirm" && <ConfirmSettings ctx={ctx} campaign={flow.campaign} onBack={() => setFlow(flow.campaign.script_id ? { step: "pc", campaign: flow.campaign } : { step: "name" })} onStart={() => void startInit(flow.campaign)} />}
              </div>
            )}
            {panel === "continue" && (
              <div className="lobby-card lobby-continue">
                {campaigns.map((c, i) => (
                  <div key={c.id} className="lobby-recent-row" style={{ animationDelay: `${i * 0.04}s` }}>
                    <button className="lobby-recent-item" onClick={() => onEnterGame(c.id, null, false)}>
                      <strong>{c.name}</strong>
                      <small>
                        {scriptName(c.script_id)} · 第 {c.turn} 回合 · {new Date(c.updated_at).toLocaleString()}
                        {c.status === "ended" ? " · 已结束" : ""}
                      </small>
                    </button>
                    <button className="lobby-del" title="删除这局游戏（移到回收站）" aria-label={`删除「${c.name}」`} onClick={() => void removeCampaign(c)}>
                      🗑
                    </button>
                  </div>
                ))}
                {campaigns.length === 0 && <p className="hint">还没有存档。</p>}
              </div>
            )}
            {panel === "settings" && <GlobalSettings lowPower={lowPower} onOpenModels={() => onOpenStudio("settings")} onOpenGeneral={() => onOpenStudio("general")} notify={notify} />}
          </>
        )}
        <div className="lobby-nav">
          {onOpenMultiplayer && (
            <button className="lobby-nav-btn" onClick={onOpenMultiplayer} data-tour="lobby-mp">
              <span className="ic">🌐</span>
              <span>
                <strong>联机游戏</strong>
                <small>开房 / 加入朋友的房间</small>
              </span>
            </button>
          )}
          <button className="lobby-nav-btn" onClick={() => onOpenStudio()}>
            <span className="ic">✏️</span>
            <span>
              <strong>创作模式</strong>
              <small>世界树、剧本、审阅……</small>
            </span>
          </button>
        </div>
      </aside>
    </div>
  );
}

/** 开局前的设置：难度（本局）、掷骰方式、自动朗读。 */
function ConfirmSettings({ ctx, campaign, onBack, onStart }: { ctx: Ctx; campaign: Campaign; onBack: () => void; onStart: () => void }) {
  const [difficulty, setDifficulty] = useState("normal");
  const [dice, setDice] = useState<"auto" | "click" | "physical">("auto");
  const [autoRead, setAutoRead] = useState(() => {
    try {
      return localStorage.getItem("autoRead") === "1";
    } catch {
      return false;
    }
  });
  const [pcName, setPcName] = useState<string | null>(null);
  useEffect(() => {
    api.rpgOverview(ctx.worldId, campaign.id).then((o) => setDifficulty(o.rules.difficulty || "normal")).catch(() => {});
    api.diceMode().then(setDice).catch(() => {});
    api.campaignPc(ctx.worldId, campaign.id).then((p) => setPcName(p ? `${p.name}（${p.identity}）` : null)).catch(() => {});
  }, [ctx.worldId, campaign.id]);
  async function go() {
    try {
      const o = await api.rpgOverview(ctx.worldId, campaign.id);
      if (o.rules.difficulty !== difficulty) await api.rpgSetRules(ctx.worldId, campaign.id, { ...o.rules, difficulty });
      await api.setDiceMode(dice);
      try {
        localStorage.setItem("autoRead", autoRead ? "1" : "0");
      } catch {
        /* 只在本次有效 */
      }
      onStart();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }
  return (
    <div className="confirm-settings">
      <p>
        <strong>{campaign.name}</strong>
        {pcName && <span className="hint"> · 你扮演 {pcName}</span>}
      </p>
      <label>
        难度
        <select value={difficulty} onChange={(e) => setDifficulty(e.target.value)}>
          <option value="easy">轻松</option>
          <option value="normal">普通</option>
          <option value="hard">困难</option>
          <option value="brutal">残酷</option>
        </select>
      </label>
      <label>
        掷骰
        <select value={dice} onChange={(e) => setDice(e.target.value as typeof dice)}>
          <option value="auto">程序自动掷</option>
          <option value="click">我来点击掷骰</option>
          <option value="physical">掷实体骰子，输入点数</option>
        </select>
      </label>
      <label className="check">
        <input type="checkbox" checked={autoRead} onChange={(e) => setAutoRead(e.target.checked)} />
        自动朗读 GM 的讲述
      </label>
      <div className="row">
        <button onClick={onBack}>上一步</button>
        <button className="primary" onClick={() => void go()} data-tour="lobby-start">
          确认，初始化世界
        </button>
      </div>
    </div>
  );
}

/** 全局设置：低性能模式、掷骰方式、模型服务入口。 */
function GlobalSettings({ lowPower, onOpenModels, onOpenGeneral, notify }: { lowPower: boolean; onOpenModels: () => void; onOpenGeneral: () => void; notify: (m: string, k?: "info" | "error") => void }) {
  const [dice, setDice] = useState<"auto" | "click" | "physical">("auto");
  useEffect(() => {
    api.diceMode().then(setDice).catch(() => {});
  }, []);
  return (
    <div className="lobby-card">
      <label className="check" title="关掉着色器玻璃球、3D 骰子、粒子和大部分动画（老电脑或用电池时）">
        <input type="checkbox" checked={lowPower} onChange={(e) => void setLowPower(e.target.checked).catch((er: Error) => notify(er.message, "error"))} />
        低性能模式
      </label>
      <label>
        掷骰
        <select
          value={dice}
          onChange={(e) => {
            const m = e.target.value as typeof dice;
            void api
              .setDiceMode(m)
              .then(() => setDice(m))
              .catch((er: Error) => notify(er.message, "error"));
          }}
        >
          <option value="auto">程序自动掷</option>
          <option value="click">我来点击掷骰</option>
          <option value="physical">掷实体骰子，输入点数</option>
        </select>
      </label>
      <button onClick={onOpenModels}>模型服务与语音…</button>
      <button onClick={onOpenGeneral}>更多设置（后台运行、开机启动、界面大小、更新…）</button>
    </div>
  );
}
