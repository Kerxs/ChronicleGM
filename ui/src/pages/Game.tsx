import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
import type { ForkPoint } from "../bindings/ForkPoint";
import type { VerifyReport } from "../bindings/VerifyReport";
import type { TimelineComparison } from "../bindings/TimelineComparison";
import { MentionThumbs, NodeGallery, ScenePictures, useIllustrated, type GalleryTarget } from "../components/SceneImages";

const TRI: Record<string, string> = { true: "条件满足", false: "条件不满足", unknown: "需检定" };

export default function GamePage({ ctx }: { ctx: Ctx }) {
  const [campaigns, setCampaigns] = useState<Campaign[]>([]);
  const [scripts, setScripts] = useState<Script[]>([]);
  const [cid, setCid] = useState<string | null>(null);
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
        <select value={newScript} onChange={(e) => setNewScript(e.target.value)}>
          <option value="">自由沙盒（不使用剧本）</option>
          {scripts.map((s) => (
            <option key={s.id} value={s.id}>
              {s.title}
            </option>
          ))}
        </select>
        <input value={newName} onChange={(e) => setNewName(e.target.value)} placeholder="战役名称" />
        <button onClick={() => void create()}>新建战役</button>
      </header>
      {cid ? <Table key={cid} ctx={ctx} campaignId={cid} campaigns={campaigns} onSwitch={(id) => { setCid(id); ctx.bump(); }} /> : <p className="hint">选择或新建一个战役开始游戏。GM 由 AI 主持；骰子、规则与状态变化都由程序判定。</p>}
    </section>
  );
}

function Table({ ctx, campaignId, campaigns, onSwitch }: { ctx: Ctx; campaignId: string; campaigns: Campaign[]; onSwitch: (id: string) => void }) {
  const [events, setEvents] = useState<EventEntry[]>([]);
  const [status, setStatus] = useState<CampaignStatus | null>(null);
  const [script, setScript] = useState<ScriptView | null>(null);
  const [memory, setMemory] = useState<CampaignMemory | null>(null);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [gmView, setGmView] = useState(false);
  const [last, setLast] = useState<TurnResult | null>(null);
  const [tab, setTab] = useState<"state" | "board" | "quests" | "npcs" | "world" | "character" | "combat" | "console" | "branches" | "memory" | "timeline">("state");
  const [draft, setDraft] = useState("");
  const bottom = useRef<HTMLDivElement>(null);
  // GM 玻璃球：构思中的阶段；正在讲述的叙述（讲完才落进剧情流）；刚落下的叙述（播放落下动画）。
  const [stage, setStage] = useState<string | undefined>(undefined);
  const [speaking, setSpeaking] = useState<string | null>(null);
  const [landing, setLanding] = useState<string | null>(null);
  /** 正在查看图片的节点（地点、人物或叙述里提到的设定）。 */
  const [gallery, setGallery] = useState<GalleryTarget | null>(null);
  const illustrated = useIllustrated(ctx.worldId, ctx.tick);

  const refresh = useCallback(async () => {
    try {
      const [ev, st, mem] = await Promise.all([
        api.campaignLog(ctx.worldId, campaignId),
        api.campaignStatus(ctx.worldId, campaignId),
        api.campaignMemory(ctx.worldId, campaignId),
      ]);
      setEvents(ev);
      setStatus(st);
      setMemory(mem);
      const d = mem.summaries.find((x) => x.status === "draft");
      setDraft(d?.summary ?? "");
      if (st.campaign.script_id && !script) setScript(await api.getScript(ctx.worldId, st.campaign.script_id));
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }, [ctx.worldId, campaignId, script]);

  useEffect(() => {
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ctx.tick]);

  useEffect(() => {
    bottom.current?.scrollIntoView({ behavior: "smooth" });
  }, [events.length]);

  const names: Names = useMemo(() => {
    const m = new Map<string, string>();
    for (const it of script?.items ?? []) m.set(it.id, it.title);
    for (const c of script?.cast ?? []) m.set(c.cast.node_id, c.name);
    return (id: string) => m.get(id) ?? id;
  }, [script]);

  const lines = useMemo(() => {
    const all = toTranscript(events).filter((l) => gmView || !l.gmOnly);
    if (!speaking) return all;
    // 正在由玻璃球讲述的叙述先不进剧情流，讲完再落下（同一段文字不会同时出现两遍）。
    for (let i = all.length - 1; i >= 0; i--) {
      if (all[i].who === "gm" && all[i].text.trim() === speaking) return all.filter((_, j) => j !== i);
    }
    return all;
  }, [events, gmView, speaking]);
  const orbState: OrbState = busy ? "thinking" : speaking ? "speaking" : "idle";
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
    void listen<{ campaign_id: string; adopted: number; trigger: string }>("director-note", (e) => {
      if (e.payload.campaign_id !== campaignId) return;
      if (gmView) ctx.notify(`导演给出了新的建议${e.payload.adopted ? `（已自动采用 ${e.payload.adopted} 条）` : ""}，见「控制台」`);
      ctx.bump();
    }).then((f) => (off = f));
    return () => off?.();
  }, [campaignId, gmView]);

  useEffect(() => {
    let off: (() => void) | undefined;
    void listen<{ campaign_id: string; stage: string }>("gm-stage", (e) => {
      if (e.payload.campaign_id === campaignId) setStage(e.payload.stage);
    }).then((f) => (off = f));
    return () => off?.();
  }, [campaignId]);

  async function send() {
    if (!input.trim() || busy) return;
    setBusy(true);
    setStage(undefined);
    setSpeaking(null);
    try {
      const r = await api.gmTurn(ctx.worldId, campaignId, input.trim());
      if (r.narration.trim()) setSpeaking(r.narration.trim());
      if (autoRead) {
        if (r.narration.trim()) voice.enqueue(null, r.narration);
        for (const n of r.npc_lines) voice.enqueue(n.npc_id, n.text);
      }
      setLast(r);
      setInput("");
      if (r.leaks.length || r.npc_lines.some((n) => n.leaks.length)) ctx.notify("本回合的文字中可能泄露了主持人信息（主持人视角可查看）", "error");
      if (r.entities.some((e) => e.entity_id)) ctx.notify("AI 提议了新的人物或地点，已加入本战役，确认后才写入世界（见审阅台）");
      if (r.combat?.status === "active") setTab("combat");
      const news = [...r.rpg_updates.slice(-3), ...(r.travel ? [r.travel] : []), ...r.places_heard.map((p) => `听说了：${p}`), ...r.discoveries.map((d) => `新发现：${d}`), ...r.quest_updates];
      if (news.length) ctx.notify(`${r.time}　${news.join("；")}`);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
      ctx.bump();
    } finally {
      setBusy(false);
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

  const st = status?.state;
  return (
    <div className="table-body">
      <div className="transcript card">
        <div className="gm-stage">
          <GmOrb
            state={orbState}
            stage={stage}
            text={speaking ?? ""}
            level={voice.level}
            onFinished={() => {
              setLanding(speaking);
              setSpeaking(null);
            }}
          />
        </div>
        <div className="lines">
          <ScenePictures ctx={ctx} campaignId={campaignId} refreshKey={events.length + ctx.tick} onOpen={setGallery} />
          {status?.current && (
            <div className="scene">
              <strong>{status.current.title}</strong>
              {status.current.player_text && <div>{status.current.player_text}</div>}
              {gmView && status.current.gm_text && <div className="gm">主持人：{status.current.gm_text}</div>}
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
          {lines.length === 0 && <p className="hint">输入玩家的第一个行动，开始游戏。</p>}
          {lines.map((l, i) => (
            <div key={`${l.seq}-${i}`} className={`line who-${l.who}${l.gmOnly ? " gm-only" : ""}${l.warn ? " warn" : ""}${landing && l.who === "gm" && l.text.trim() === landing ? " landing" : ""}`}>
              {l.who === "npc" && <span className="speaker">{l.name}：</span>}
              {l.who === "player" && <span className="speaker">玩家：</span>}
              {l.text}
              {(l.who === "npc" || l.who === "gm") && (
                <button className="link speak" title="朗读" onClick={() => voice.enqueue(l.who === "npc" ? l.speaker || null : null, l.text)}>
                  🔊
                </button>
              )}
              {(l.who === "npc" || l.who === "gm") && <MentionThumbs ctx={ctx} text={l.text} nodes={illustrated} onOpen={setGallery} />}
            </div>
          ))}
          {busy && <div className="line who-system">GM 正在构思，本地模型可能需要一段时间（可以点输入框旁的「取消」）</div>}
          <div ref={bottom} />
        </div>
        <form
          className="row input-row"
          onSubmit={(e) => {
            e.preventDefault();
            void send();
          }}
        >
          <textarea
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

      <div className="detail">
        <div className="row">
          <label className="check">
            <input type="checkbox" checked={gmView} onChange={(e) => { setGmView(e.target.checked); if (!e.target.checked && tab === "console") setTab("state"); }} />
            主持人视角（显示秘密、AI 意图、被拒绝的修改）
          </label>
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
        </div>
        <div className="tabs">
          {(
            [
              ["state", "状态"],
              ["branches", "剧情分支"],
              ["memory", "记忆"],
              ["board", "调查"],
              ["quests", "任务"],
              ["npcs", "人物"],
              ["world", "世界"],
              ["character", "角色"],
              ["combat", "战斗"],
              ["timeline", "时间线"],
              ["console", "控制台"],
            ] as const
          ).filter(([k]) => k !== "console" || gmView).map(([k, v]) => (
            <button key={k} className={tab === k ? "active" : ""} onClick={() => setTab(k)}>
              {v}
            </button>
          ))}
        </div>
        {tab === "state" && st && (
          <div className="card state">
            <div>回合 {status?.campaign.turn}</div>
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
            {gmView && last && (
              <div className="last-turn">
                <strong>上一回合（程序执行）</strong>
                <div>意图：{last.intent || "—"}{last.off_script && "（剧本外行动）"}</div>
                {last.checks.map((c, i) => (
                  <div key={i}>
                    🎲 {c.skill}：{c.roll}+{c.modifier}={c.total} / DC {c.dc} → {c.success ? "成功" : "失败"}
                  </div>
                ))}
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
            {memory.summaries.map((s) =>
              s.status === "draft" ? (
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
                    <li key={i}>{f}</li>
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
        {tab === "timeline" && status && <TimelinePanel ctx={ctx} campaign={status.campaign} campaigns={campaigns} onSwitch={onSwitch} onChanged={() => { void refresh(); ctx.bump(); }} />}
      </div>
      {gallery && <NodeGallery ctx={ctx} target={gallery} gmView={gmView} onClose={() => setGallery(null)} />}
    </div>
  );
}

const CP_LABEL: Record<string, string> = { start: "战役开始", turn_end: "回合结束", transition: "剧情推进", ending: "战役结束", fork: "时间线起点", migrated: "升级时补建" };

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

  useEffect(() => {
    if (ended) api.timelineForkPoints(ctx.worldId, campaign.id).then(setPoints).catch((e: Error) => ctx.notify(e.message, "error"));
  }, [ctx.worldId, campaign.id, ended]);

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
                  <span className="kind">第 {p.checkpoint.turn} 回合 · {CP_LABEL[p.checkpoint.label] ?? p.checkpoint.label}</span>
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
  const [playing, setPlaying] = useState(false);
  const pump = useCallback(async () => {
    if (running.current) return;
    running.current = true;
    setPlaying(true);
    try {
      while (queue.current.length) {
        const item = queue.current.shift()!;
        try {
          const r = await api.voiceSpeak(ctx.worldId, item.speaker, item.text);
          if (!running.current) break;
          await new Promise<void>((resolve) => {
            const a = new Audio(r.data_url);
            audio.current = a;
            try {
              if (!ac.current) {
                const c = new AudioContext();
                const an = c.createAnalyser();
                an.fftSize = 1024;
                an.connect(c.destination);
                ac.current = { ctx: c, an, buf: new Uint8Array(an.fftSize) };
              }
              ac.current.ctx.createMediaElementSource(a).connect(ac.current.an);
              void ac.current.ctx.resume();
            } catch {
              /* 音量分析不可用时只影响玻璃球的起伏，不影响朗读 */
            }
            a.onended = () => resolve();
            a.onerror = () => resolve();
            void a.play().catch(() => resolve());
          });
        } catch (e) {
          ctx.notify((e as Error).message, "error");
          queue.current = [];
        }
      }
    } finally {
      running.current = false;
      audio.current = null;
      setPlaying(false);
    }
  }, [ctx]);
  const enqueue = useCallback(
    (speaker: string | null, text: string) => {
      if (!text.trim()) return;
      queue.current.push({ speaker, text });
      void pump();
    },
    [pump],
  );
  const stop = useCallback(() => {
    queue.current = [];
    running.current = false;
    audio.current?.pause();
    setPlaying(false);
  }, []);
  return { enqueue, stop, playing, level };
}
