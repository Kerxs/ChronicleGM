// 游戏页的面板：调查板、任务、人物（日程与情绪）、世界（环境、地图、旅行、势力、世界事件）。只调用后端命令，规则都在 Rust 里。
import type { NpcMemory } from "../bindings/NpcMemory";
import type { QuestProposal } from "../bindings/QuestProposal";
import { PanelSkeleton } from "./Skeleton";
import { useEffect, useState } from "react";
import type React from "react";
import { api, type NpcStates, type WorldSimOverview } from "../api";
import type { TravelPlan } from "../bindings/TravelPlan";
import type { MapPlace } from "../bindings/MapPlace";
import { MapView } from "./MapView";
import type { Ctx } from "../App";
import type { Board } from "../bindings/Board";
import type { QuestView } from "../bindings/QuestView";
import type { Condition } from "../bindings/Condition";
import { describeCondition, type Names } from "../lib/dsl";

const SOURCE: Record<string, string> = { script: "线索", gm: "发现", player: "我的笔记" };
const REL: Record<string, string> = { supports: "支持", contradicts: "矛盾", related: "相关" };
const HSTATUS: Record<string, string> = { open: "还在考虑", believed: "我相信", rejected: "我否定" };

/** 调查板：线索、关联、玩家自己的推理与证据链。推理对错由玩家自己判断。 */
export function InvestigationPanel({ ctx, campaignId, gmView, ended, refreshKey }: { ctx: Ctx; campaignId: string; gmView: boolean; ended: boolean; refreshKey: number }) {
  const [b, setB] = useState<Board | null>(null);
  const [title, setTitle] = useState("");
  const [detail, setDetail] = useState("");
  const [la, setLa] = useState("");
  const [lb, setLb] = useState("");
  const [rel, setRel] = useState("supports");
  const [hText, setHText] = useState("");
  const [hEv, setHEv] = useState<string[]>([]);
  const load = () => api.investigationBoard(ctx.worldId, campaignId, gmView).then(setB).catch((e: Error) => ctx.notify(e.message, "error"));
  useEffect(() => void load(), [campaignId, gmView, refreshKey]);
  const run = (f: () => Promise<unknown>) => void f().then(load).catch((e: Error) => ctx.notify(e.message, "error"));
  if (!b) return <PanelSkeleton />;
  const name = (id: string) => b.clues.find((c) => c.id === id)?.title ?? "（已删除）";
  return (
    <div className="card board">
      <h3>
        线索（{b.clues.length}）
        {b.clues.some((c) => c.unresolved) && <small className="hint">　未解决 {b.clues.filter((c) => c.unresolved).length} 条（还没被任何推理用到）</small>}
      </h3>
      {b.clues.length === 0 && <p className="hint">还没有线索。剧本线索被揭示、GM 记录的新发现会自动出现在这里，你也可以记自己的笔记。</p>}
      <ul className="list">
        {b.clues.map((c) => (
          <li key={c.id}>
            <span className={`kind src-${c.source}`}>{SOURCE[c.source] ?? c.source}</span>
            <strong>{c.title}</strong> <small className="hint">第 {c.turn + 1} 回合</small>
            {c.unresolved && <span className="badge">未解决</span>}
            {c.detail && <div>{c.detail}</div>}
            {(c.found_at || c.related.length > 0) && (
              <div className="hint">
                {c.found_at && <>发现于：{c.found_at}　</>}
                {c.related.length > 0 && <>相关人物：{c.related.join("、")}</>}
              </div>
            )}
            {gmView && c.gm_note && <div className="gm">主持人：{c.gm_note}</div>}
            {c.source === "player" && !ended && <button className="link danger" onClick={() => run(() => api.boardRemoveNote(ctx.worldId, campaignId, c.id))}>删除</button>}
          </li>
        ))}
      </ul>
      {!ended && (
        <div className="row wrap">
          <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="记一条笔记（标题）" />
          <input value={detail} onChange={(e) => setDetail(e.target.value)} placeholder="内容（可空）" style={{ flex: 1 }} />
          <button disabled={!title.trim()} onClick={() => run(async () => { await api.boardAddNote(ctx.worldId, campaignId, title, detail); setTitle(""); setDetail(""); })}>记下</button>
        </div>
      )}

      {gmView && b.hidden.length > 0 && (
        <div className="gm">
          <strong>还没发现的线索（{b.hidden.length}，剧透）</strong>
          <ul className="list">
            {b.hidden.map((h) => (
              <li key={h.id}>
                {h.title}
                <small className="hint">
                  {h.found_at && `　在：${h.found_at}`}
                  {h.related.length > 0 && `　相关：${h.related.join("、")}`}
                  {h.unlock && `　解锁：${h.unlock}`}
                </small>
              </li>
            ))}
          </ul>
        </div>
      )}

      <h3>关联（{b.links.length}）</h3>
      <ul className="list">
        {b.links.map((l) => (
          <li key={l.id}>
            「{name(l.a)}」<span className={`kind rel-${l.relation}`}>{REL[l.relation] ?? l.relation}</span>「{name(l.b)}」{l.note && <span className="hint">：{l.note}</span>}
            {!ended && <button className="link" onClick={() => run(() => api.boardUnlink(ctx.worldId, campaignId, l.id))}>取消</button>}
          </li>
        ))}
      </ul>
      {!ended && b.clues.length >= 2 && (
        <div className="row wrap">
          <select value={la} onChange={(e) => setLa(e.target.value)}>
            <option value="">选择线索</option>
            {b.clues.map((c) => <option key={c.id} value={c.id}>{c.title}</option>)}
          </select>
          <select value={rel} onChange={(e) => setRel(e.target.value)}>
            {Object.entries(REL).map(([k, v]) => <option key={k} value={k}>{v}</option>)}
          </select>
          <select value={lb} onChange={(e) => setLb(e.target.value)}>
            <option value="">选择线索</option>
            {b.clues.map((c) => <option key={c.id} value={c.id}>{c.title}</option>)}
          </select>
          <button disabled={!la || !lb || la === lb} onClick={() => run(() => api.boardLink(ctx.worldId, campaignId, la, lb, rel, ""))}>关联</button>
        </div>
      )}

      <h3>我的推理（{b.hypotheses.length}）</h3>
      <p className="hint">推理是你自己的判断：程序不会告诉你对不对，GM 也不会替你揭晓答案。</p>
      <ul className="list">
        {b.hypotheses.map((h) => (
          <li key={h.id}>
            <strong>{h.text}</strong>{" "}
            <select
              value={h.status}
              disabled={ended}
              onChange={(e) => run(() => api.boardSaveHypothesis(ctx.worldId, campaignId, h.id, { text: h.text, evidence: h.evidence, against: h.against, status: e.target.value }))}
            >
              {Object.entries(HSTATUS).map(([k, v]) => <option key={k} value={k}>{v}</option>)}
            </select>
            <div className="hint">直接依据：{h.evidence.map(name).join("、") || "无"}</div>
            {h.indirect.length > 0 && <div className="hint">证据链（通过"支持"关联连起来的）：{h.indirect.map(name).join(" → ")}</div>}
            {h.conflicts.length > 0 && <div className="warn-text">与证据链矛盾的线索：{h.conflicts.map(name).join("、")}</div>}
            {!ended && <button className="link danger" onClick={() => run(() => api.boardDeleteHypothesis(ctx.worldId, campaignId, h.id))}>删除</button>}
          </li>
        ))}
      </ul>
      {!ended && (
        <div>
          <input value={hText} onChange={(e) => setHText(e.target.value)} placeholder="写下你的推理，例如：铁匠偷了钥匙" style={{ width: "100%" }} />
          <div className="row wrap">
            {b.clues.map((c) => (
              <label key={c.id} className="check">
                <input type="checkbox" checked={hEv.includes(c.id)} onChange={(e) => setHEv((v) => (e.target.checked ? [...v, c.id] : v.filter((x) => x !== c.id)))} />
                {c.title}
              </label>
            ))}
          </div>
          <button disabled={!hText.trim()} onClick={() => run(async () => { await api.boardSaveHypothesis(ctx.worldId, campaignId, null, { text: hText, evidence: hEv, against: [], status: "open" }); setHText(""); setHEv([]); })}>
            提出推理
          </button>
        </div>
      )}
    </div>
  );
}

const QTYPE: Record<string, string> = { main: "主线", side: "支线", hidden: "隐藏" };
const QSTATUS: Record<string, string> = { "": "未开始", active: "进行中", done: "已完成", failed: "已失败" };

type QuestMode = "current" | "all" | "hidden";
const QMODES: [QuestMode, string][] = [
  ["current", "当前任务"],
  ["all", "全部已解锁"],
  ["hidden", "不显示"],
];

function QuestGenerator({ ctx, campaignId, onAdded }: { ctx: Ctx; campaignId: string; onAdded: () => void }) {
  const [open, setOpen] = useState(false);
  const [hint, setHint] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [prop, setProp] = useState<QuestProposal | null>(null);
  async function gen(mode: "situation" | "worldview") {
    setBusy(mode);
    setProp(null);
    try {
      setProp(await api.generateQuest(ctx.worldId, campaignId, mode, hint));
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    } finally {
      setBusy(null);
    }
  }
  if (!open)
    return (
      <button className="small" onClick={() => setOpen(true)} data-tour="quest-gen">
        生成任务…
      </button>
    );
  return (
    <div className="quest-gen card">
      <div className="row">
        <input value={hint} onChange={(e) => setHint(e.target.value)} placeholder="想要什么样的任务？（可选）" disabled={busy !== null} />
        <button disabled={busy !== null} onClick={() => void gen("situation")} title="结合当前场景、刚做过的事、人物关系和世界状态">
          {busy === "situation" ? "生成中…" : "根据当前剧情"}
        </button>
        <button disabled={busy !== null} onClick={() => void gen("worldview")} title="结合主线、背景事件和尚未触发的内容">
          {busy === "worldview" ? "生成中…" : "根据剧本世界观"}
        </button>
        <button className="link" onClick={() => setOpen(false)}>
          收起
        </button>
      </div>
      {prop && (
        <div className="proposal">
          <strong>{prop.quest.title}</strong> <span className="hint">{prop.quest.quest_type === "main" ? "主线" : "支线"} · {prop.mode === "situation" ? "根据当前剧情" : "根据剧本世界观"}</span>
          {prop.quest.background && <p>{prop.quest.background}</p>}
          <div className="bs-row"><span className="bs-k">目标</span><span>{prop.quest.objective || "—"}</span></div>
          <div className="bs-row"><span className="bs-k">完成条件</span><span>{prop.quest.completion || "—"}</span></div>
          <div className="bs-row gm"><span className="bs-k">后果</span><span>{prop.quest.consequences || "—"}</span></div>
          {(prop.npc_names.length > 0 || prop.location_name) && (
            <div className="hint">
              {prop.npc_names.length > 0 && <>相关人物：{prop.npc_names.join("、")}　</>}
              {prop.location_name && <>地点：{prop.location_name}</>}
            </div>
          )}
          {prop.conflicts.length > 0 ? (
            <ul className="findings">
              {prop.conflicts.map((c, i) => (
                <li key={i} className="finding sev-warning">
                  <span className="sev">注意</span>
                  <div>{c}</div>
                </li>
              ))}
            </ul>
          ) : (
            <p className="hint">程序检查：没有发现和已有事实的冲突。</p>
          )}
          <div className="row">
            <button
              className="primary"
              onClick={() =>
                void api
                  .questAdd(ctx.worldId, campaignId, prop.quest)
                  .then(() => {
                    ctx.notify("已加入任务「" + prop.quest.title + "」");
                    setProp(null);
                    onAdded();
                  })
                  .catch((e: Error) => ctx.notify(e.message, "error"))
              }
            >
              加入任务
            </button>
            <button onClick={() => setProp(null)}>不要</button>
          </div>
        </div>
      )}
    </div>
  );
}

export function QuestPanel({ ctx, campaignId, gmView, names, refreshKey }: { ctx: Ctx; campaignId: string; gmView: boolean; names: Names; refreshKey: number }) {
  const [qs, setQs] = useState<QuestView[]>([]);
  const [mode, setMode] = useState<QuestMode>(() => {
    try {
      return (localStorage.getItem("questMode") as QuestMode) || "all";
    } catch {
      return "all";
    }
  });
  useEffect(() => {
    api.questList(ctx.worldId, campaignId, gmView).then(setQs).catch((e: Error) => ctx.notify(e.message, "error"));
  }, [campaignId, gmView, refreshKey]);
  const cond = (c: unknown) => (c ? describeCondition(c as Condition, names) : null);
  const pick = (m: QuestMode) => {
    setMode(m);
    try {
      localStorage.setItem("questMode", m);
    } catch {
      /* 浏览器存储不可用时只在本次有效 */
    }
  };
  // 主持人视角显示全部；玩家视角按模式筛选（隐藏只影响显示，任务照常运行）。
  const shown = gmView ? qs : mode === "hidden" ? [] : mode === "current" ? qs.filter((q) => q.status === "active") : qs.filter((q) => q.status !== "");
  return (
    <div className="card">
      <div className="row quest-head">
        <div className="seg-mini">
          {QMODES.map(([m, label]) => (
            <button key={m} className={mode === m ? "on" : ""} onClick={() => pick(m)}>
              {label}
            </button>
          ))}
        </div>
        <QuestGenerator ctx={ctx} campaignId={campaignId} onAdded={ctx.bump} />
      </div>
      {!gmView && mode === "hidden" && <p className="hint">任务信息已隐藏，任务照常在后台推进。</p>}
      {shown.length === 0 && (gmView || mode !== "hidden") && <p className="hint">{mode === "current" && !gmView ? "现在没有进行中的任务。" : "还没有任务。"}</p>}
      <ul className="list quests">
        {shown.map((q) => (
          <li key={q.id} className={`q-${q.status || "none"}`}>
            <span className="kind">{QTYPE[q.quest_type] ?? q.quest_type}</span>
            <strong>{q.title}</strong> <span className="hint">{QSTATUS[q.status] ?? q.status}{q.source === "gm" && " · 游戏中出现"}{q.source === "generated" && " · AI 生成"}</span>
            {q.turns_left !== null && <span className={q.turns_left <= 1 ? "warn-text" : "hint"}> · 还剩 {q.turns_left} 回合</span>}
            {q.minutes_left !== null && q.minutes_left !== undefined && (
              <span className={q.minutes_left <= 120 ? "warn-text" : "hint"}> · 故事时间还剩 {q.minutes_left >= 1440 ? `${(q.minutes_left / 1440).toFixed(1)} 天` : `${(q.minutes_left / 60).toFixed(1)} 小时`}</span>
            )}
            {q.description && <div>{q.description}</div>}
            {q.objectives.length > 0 && <ul>{q.objectives.map((o, i) => <li key={i}>{o}</li>)}</ul>}
            <div className="hint">
              {q.npcs.length > 0 && <>相关人物：{q.npcs.map((n) => n.title).join("、")}　</>}
              {q.location && <>地点：{q.location.title}　</>}
              {q.clues.length > 0 && <>线索：{q.clues.map((c) => `${c.title}${q.known_clues.includes(c.id) ? "✓" : ""}`).join("、")}</>}
            </div>
            {gmView && (
              <div className="gm">
                {cond(q.precondition) && <div>开始条件：{cond(q.precondition)}</div>}
                {cond(q.complete_when) && <div>完成条件：{cond(q.complete_when)}</div>}
                {cond(q.fail_when) && <div>失败条件：{cond(q.fail_when)}</div>}
                {q.deadline_turns && <div>期限：开始后 {q.deadline_turns} 回合</div>}
              </div>
            )}
            {q.completion && <div className="hint">完成条件：{q.completion}</div>}
            {gmView && q.consequences && <div className="gm">后果：{q.consequences}</div>}
            {q.notes.length > 0 && (
              <div className="hint">
                变化：{q.notes.map((n) => `第 ${n.turn + 1} 回合 ${n.text}${n.gm_only ? "（仅主持人可见）" : ""}`).join("；")}
              </div>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

const MOOD_LABEL: [keyof NonNullable<NpcStates["npcs"][number]["mood"]>, string][] = [
  ["fear", "恐惧"],
  ["anger", "愤怒"],
  ["suspicion", "怀疑"],
  ["stress", "压力"],
  ["loyalty", "忠诚"],
];

/** NPC 记住的事（剧透模式下可以查看、删除；删掉的内容留在事件日志里）。 */
function NpcMemories({ ctx, campaignId, npcId }: { ctx: Ctx; campaignId: string; npcId: string }) {
  const [open, setOpen] = useState(false);
  const [list, setList] = useState<NpcMemory[]>([]);
  const load = () => void api.npcMemoryList(ctx.worldId, campaignId, npcId).then(setList).catch((e: Error) => ctx.notify(e.message, "error"));
  if (!open)
    return (
      <button
        className="link small"
        onClick={() => {
          setOpen(true);
          load();
        }}
      >
        它记得的事…
      </button>
    );
  return (
    <ul className="npc-mem">
      {list.length === 0 && <li className="hint">没有记忆。</li>}
      {list.map((m) => (
        <li key={m.id}>
          {m.content} <small className="hint">（重要度 {m.importance}，第 {m.turn} 回合）</small>{" "}
          <button className="link" onClick={() => void api.deleteNpcMemory(ctx.worldId, campaignId, m.id).then(load).catch((e: Error) => ctx.notify(e.message, "error"))}>
            删除
          </button>
        </li>
      ))}
    </ul>
  );
}

export function NpcPanel({ ctx, campaignId, gmView, ended, refreshKey, onChanged }: { ctx: Ctx; campaignId: string; gmView: boolean; ended: boolean; refreshKey: number; onChanged: () => void }) {
  const [st, setSt] = useState<NpcStates | null>(null);
  useEffect(() => {
    api.npcStates(ctx.worldId, campaignId).then(setSt).catch((e: Error) => ctx.notify(e.message, "error"));
  }, [campaignId, refreshKey]);
  if (!st) return <PanelSkeleton />;
  const wait = (m: number) =>
    void api
      .advanceStoryTime(ctx.worldId, campaignId, m)
      .then((notes) => {
        ctx.notify(notes.length ? notes.join("；") : `过去了 ${m} 分钟`);
        onChanged();
      })
      .catch((e: Error) => ctx.notify(e.message, "error"));
  return (
    <div className="card">
      <div className="row wrap">
        <strong>{st.time}</strong>
        {!ended && (
          <>
            <span className="hint">等待：</span>
            {[
              [30, "半小时"],
              [120, "两小时"],
              [480, "到下一个时段"],
            ].map(([m, l]) => (
              <button key={m} onClick={() => wait(m as number)}>{l}</button>
            ))}
          </>
        )}
      </div>
      {st.npcs.length === 0 && <p className="hint">剧本人物表里还没有人物。</p>}
      <ul className="list">
        {st.npcs.map((n) => (
          <li key={n.status.npc_id}>
            <strong>{n.status.name}</strong>
            {n.status.life_status !== "alive" && <span className="warn-text">（{n.status.life_status}）</span>}
            <div className="hint">
              {n.status.activity ? `${n.status.activity}${n.status.location_name ? `（${n.status.location_name}）` : ""}` : n.status.has_schedule ? "这个时间没有安排" : "没有设定日程"}
            </div>
            {gmView && n.mood && (
              <div className="moods">
                {MOOD_LABEL.map(([k, l]) => (
                  <span key={k} className={n.mood![k] >= 70 ? "warn-text" : ""}>{l} {n.mood![k]}　</span>
                ))}
              </div>
            )}
            {gmView && n.bond_to_party && (
              <div className="hint">对玩家：信任 {n.bond_to_party.trust}，好感 {n.bond_to_party.affection}，敌意 {n.bond_to_party.hostility}</div>
            )}
            {gmView && <NpcMemories ctx={ctx} campaignId={campaignId} npcId={n.status.npc_id} />}
          </li>
        ))}
      </ul>
      {!gmView && <p className="hint">打开"主持人视角"可以看到人物的情绪与对玩家的关系。</p>}
    </div>
  );
}

const PLACE_STATE: Record<string, string> = { explored: "已探索", heard: "未探索", unknown: "未知" };
const PLACE_KIND: Record<string, string> = { region: "区域", nation: "国家", city: "城市", site: "地点" };
const WEATHERS = ["晴", "多云", "小雨", "暴雨", "雾", "雪", "暴雪"];
const POWER_LABEL: [keyof WorldSimOverview["factions"][number]["power"], string][] = [
  ["wealth", "财富"],
  ["military", "军力"],
  ["influence", "影响"],
  ["stability", "稳定"],
];

function duration(m: number): string {
  if (m < 60) return `${m} 分钟`;
  if (m < 1440) return `${(m / 60).toFixed(m % 60 ? 1 : 0)} 小时`;
  return `${(m / 1440).toFixed(1)} 天`;
}

/** 世界：环境（昼夜、季节、天气）、地图（探索状态与标记）、旅行、势力、世界事件。全部由程序计算。 */
export function WorldPanel({ ctx, campaignId, gmView, ended, refreshKey, onChanged }: { ctx: Ctx; campaignId: string; gmView: boolean; ended: boolean; refreshKey: number; onChanged: () => void }) {
  const [ov, setOv] = useState<WorldSimOverview | null>(null);
  const [plan, setPlan] = useState<TravelPlan | null>(null);
  const [busy, setBusy] = useState(false);
  const [marker, setMarker] = useState({ place: "", label: "", note: "" });
  const [mapMode, setMapMode] = useState<"map" | "list">("map");
  const load = () => api.worldSimOverview(ctx.worldId, campaignId, gmView).then(setOv).catch((e: Error) => ctx.notify(e.message, "error"));
  useEffect(() => {
    void load();
    setPlan(null);
  }, [campaignId, refreshKey, gmView]);
  if (!ov) return <PanelSkeleton />;
  const env = ov.environment;
  const byId = new Map(ov.map.places.map((p) => [p.id, p]));
  const dist = new Map(ov.reachable.map((r) => [r.id, Number(r.base_minutes)]));
  const children = (parent: string | null) => ov.map.places.filter((p) => (p.parent_id && byId.has(p.parent_id) ? p.parent_id : null) === parent);
  const fname = (id: string) => ov.factions.find((f) => f.id === id)?.name ?? id;
  const fail = (e: Error) => ctx.notify(e.message, "error");
  const preview = (to: string) => void api.planTravel(ctx.worldId, campaignId, to).then(setPlan).catch(fail);
  const go = (to: string) => {
    setBusy(true);
    api
      .travelTo(ctx.worldId, campaignId, to)
      .then((r) => {
        ctx.notify(`到达「${r.plan.to_title}」，路上 ${duration(Number(r.plan.minutes))}${r.notes.length ? "。" + r.notes.slice(0, 6).join("；") : ""}`);
        setPlan(null);
        onChanged();
      })
      .catch(fail)
      .finally(() => setBusy(false));
  };
  const addMarker = () =>
    void api
      .addMapMarker(ctx.worldId, campaignId, marker.place, marker.label, marker.note)
      .then(() => {
        setMarker({ place: marker.place, label: "", note: "" });
        void load();
      })
      .catch(fail);
  const placeRow = (p: MapPlace, depth: number): React.ReactElement => (
    <li key={p.id} style={{ marginLeft: depth * 16 }}>
      <span className={`badge state-${p.state}`}>{PLACE_STATE[p.state] ?? p.state}</span> <strong>{p.title}</strong>
      <span className="hint">（{PLACE_KIND[p.kind] ?? p.kind}{dist.has(p.id) ? `，路程 ${duration(dist.get(p.id)!)}` : ""}）</span>
      {p.here && <span className="ok-text">　← 你在这里</span>}
      {!p.here && !ended && dist.has(p.id) && (
        <>
          {" "}
          <button className="small" onClick={() => preview(p.id)}>路线</button>
        </>
      )}
      {p.markers.map((m) => (
        <div key={m.id} className="hint marker">
          📍 {m.label}
          {m.note ? `：${m.note}` : ""}
          {!ended && (
            <button className="small link" onClick={() => void api.removeMapMarker(ctx.worldId, campaignId, m.id).then(load).catch(fail)}>删除</button>
          )}
        </div>
      ))}
      {children(p.id).length > 0 && <ul className="list plain">{children(p.id).map((c) => placeRow(c, 1))}</ul>}
    </li>
  );
  return (
    <div className="world-panel">
      <div className="card">
        <div className="row wrap">
          <strong>{env.time}</strong>
          <span>{env.season}季</span>
          <span className={env.bad_weather ? "warn-text" : ""}>天气：{env.weather}</span>
          {env.travel_factor > 1 && <span className="hint">赶路耗时 ×{env.travel_factor.toFixed(2)}</span>}
        </div>
        {env.bad_weather && <p className="hint">恶劣天气：户外活动的人物会待在家里，势力暂停作战。</p>}
        {gmView && !ended && (
          <div className="row wrap">
            <span className="hint">主持人指定今天的天气：</span>
            <select defaultValue="" onChange={(e) => void api.setWeatherToday(ctx.worldId, campaignId, e.target.value).then(onChanged).catch(fail)}>
              <option value="">（按季节随机）</option>
              {WEATHERS.map((w) => (
                <option key={w} value={w}>{w}</option>
              ))}
            </select>
          </div>
        )}
      </div>

      {plan && (
        <div className="card">
          <strong>前往「{plan.to_title}」</strong>
          <div className="hint">
            {plan.legs.length === 0 ? "还不知道出发地点，直接到达。" : plan.legs.map((l) => `${l.via} ${duration(Number(l.minutes))} → ${l.to_title}`).join("，")}
          </div>
          <div>
            路程 {duration(Number(plan.base_minutes))}
            {plan.factor !== 1 && `，${plan.weather}、${plan.daypart} ×${plan.factor.toFixed(2)}`}，共 <strong>{duration(Number(plan.minutes))}</strong>
          </div>
          <div className="row">
            <button className="primary" disabled={busy} onClick={() => go(plan.to)}>出发</button>
            <button onClick={() => setPlan(null)}>取消</button>
          </div>
          <p className="hint">路上世界照常运转：人物按日程活动，势力按天行动。</p>
        </div>
      )}

      <div className="card">
        <strong>地图</strong>
        <span className="hint">
          　
          <button className={`small link${mapMode === "map" ? " active" : ""}`} onClick={() => setMapMode("map")}>图形</button>
          <button className={`small link${mapMode === "list" ? " active" : ""}`} onClick={() => setMapMode("list")}>列表</button>
        </span>
        {ov.map.places.length === 0 ? (
          <p className="hint">世界树里还没有区域、国家、城市或地点，或者玩家还没听说过任何地方。用"通往""接壤"关系连接地点，说明里写耗时（例如"2小时"）。</p>
        ) : mapMode === "map" ? (
          <>
            <MapView places={ov.map.places} roads={ov.map.roads} onPick={(id) => !ended && preview(id)} />
            <p className="hint">实心：去过；虚线：听说过；半透明：玩家还不知道（只在主持人视角）。点地点查看路线。</p>
          </>
        ) : (
          <ul className="list plain">{children(null).map((p) => placeRow(p, 0))}</ul>
        )}
        {!gmView && <p className="hint">只显示去过或听说过的地方。</p>}
        {!ended && ov.map.places.length > 0 && (
          <div className="row wrap">
            <select value={marker.place} onChange={(e) => setMarker({ ...marker, place: e.target.value })}>
              <option value="">选择地点…</option>
              {ov.map.places.map((p) => (
                <option key={p.id} value={p.id}>{p.title}</option>
              ))}
            </select>
            <input placeholder="标记" value={marker.label} onChange={(e) => setMarker({ ...marker, label: e.target.value })} />
            <input placeholder="备注（可选）" value={marker.note} onChange={(e) => setMarker({ ...marker, note: e.target.value })} />
            <button disabled={!marker.place || !marker.label.trim()} onClick={addMarker}>添加标记</button>
          </div>
        )}
      </div>

      {gmView && (
        <div className="card">
          <strong>势力（主持人信息）</strong>
          {ov.factions.length === 0 && <p className="hint">世界树里没有势力。势力资料里可以设定目标与初始实力（财富、军力、影响、稳定，0–100）。</p>}
          <ul className="list">
            {ov.factions.map((f) => (
              <li key={f.id}>
                <strong>{f.name}</strong>
                {f.last_action && <span className="hint">　最近：{f.last_action}</span>}
                {f.goals.length > 0 && <div className="hint">目标：{f.goals.join("、")}</div>}
                <div className="power">
                  {POWER_LABEL.map(([k, l]) => (
                    <span key={k} className="power-item">
                      {l}
                      <span className="bar">
                        <span style={{ width: `${Number(f.power[k])}%` }} />
                      </span>
                      {Number(f.power[k])}
                    </span>
                  ))}
                </div>
                {f.at_war_with.length > 0 && <div className="warn-text">交战：{f.at_war_with.map(fname).join("、")}</div>}
                {f.allies.length > 0 && <div className="hint">同盟：{f.allies.map(fname).join("、")}</div>}
                {f.trade_with.length > 0 && <div className="hint">贸易：{f.trade_with.map(fname).join("、")}</div>}
              </li>
            ))}
          </ul>
        </div>
      )}

      <div className="card">
        <strong>世界动态</strong>
        {!gmView && <p className="hint">玩家不在场时发生的事。游戏中要靠传闻、告示或人物的闲谈才能得知。</p>}
        {ov.events.length === 0 && <p className="hint">还没有发生什么。时间推进时，势力按天行动，剧本里的触发器满足条件就会触发。</p>}
        <ul className="list">
          {ov.events.map((e) => (
            <li key={e.id}>
              <span className="hint">第 {Number(e.day)} 天</span>　{e.text}
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
