// GM 控制台（只在主持人视角显示）：剧情导演、冲突提示、当前场景、世界状态、人物、隐藏剧情、线索与触发条件、
// AI 决策与记忆检索记录、手动修改游戏状态。数据由后端计算，这里只显示与发出操作。
import { PanelSkeleton } from "./Skeleton";
import { useEffect, useState } from "react";
import { api } from "../api";
import type { Ctx } from "../App";
import type { GmConsole } from "../bindings/GmConsole";
import type { DirectorSettings } from "../bindings/DirectorSettings";
import type { ContextTrace } from "../bindings/ContextTrace";
import type { Effect } from "../bindings/Effect";
import { EffectsEditor, type Choices } from "./DslEditors";

const PACE: Record<string, string> = { raise: "提高紧张度", hold: "保持节奏", release: "放缓节奏" };
const KIND: Record<string, string> = { hint: "提示", complication: "麻烦", twist: "转折", npc_action: "人物行动", event: "事件", pacing: "节奏" };
const TRIGGER: Record<string, string> = { stall: "停滞", repeat: "重复", periodic: "例行", manual: "手动" };
const STATUS: Record<string, string> = { new: "待定", adopted: "已采用", dismissed: "已忽略", done: "已完成" };
const SEC: Record<string, string> = { kept: "放入", partial: "部分", dropped: "省略" };

function Section({ title, children, open = true }: { title: string; children: React.ReactNode; open?: boolean }) {
  const [o, setO] = useState(open);
  return (
    <div className="card console-section">
      <button className="link section-title" onClick={() => setO(!o)}>
        {o ? "▾" : "▸"} {title}
      </button>
      {o && <div>{children}</div>}
    </div>
  );
}

const DEV: Record<string, string> = { script: "偏离剧本", world: "世界观", character: "人物设定", event: "已发生事件" };

export function ConsolePanel({ ctx, campaignId, ended, refreshKey, choices, onChanged }: { ctx: Ctx; campaignId: string; ended: boolean; refreshKey: number; choices: Choices; onChanged: () => void }) {
  const [con, setCon] = useState<GmConsole | null>(null);
  const [busy, setBusy] = useState(false);
  const [effects, setEffects] = useState<Effect[]>([]);
  const [reason, setReason] = useState("");
  const [settings, setSettings] = useState<DirectorSettings | null>(null);
  const [openTurn, setOpenTurn] = useState<number | null>(null);
  const load = () => api.gmConsole(ctx.worldId, campaignId).then(setCon).catch((e: Error) => ctx.notify(e.message, "error"));
  useEffect(() => {
    void load();
  }, [campaignId, refreshKey]);
  if (!con) return <PanelSkeleton />;
  const fail = (e: Error) => ctx.notify(e.message, "error");
  const p = con.pacing;
  const setStatus = (note: string, s: string, status: string) =>
    void api
      .directorSetStatus(ctx.worldId, campaignId, note, s, status)
      .then(() => {
        void load();
        onChanged();
      })
      .catch(fail);
  const maxT = Math.max(100, ...p.curve.map((b) => b.tension));
  return (
    <div className="console-panel">
      <Section title={`剧情导演${p.stalled ? "（停滞）" : ""}`}>
        <div className="row wrap">
          <span className="badge" title={p.phase.reason}>
            阶段：{p.phase.label}（进度 {p.phase.progress}%）
          </span>
          <span>
            紧张度 <strong>{p.tension}</strong>/100
          </span>
          <span className={p.stalled ? "warn-text" : "hint"}>已经 {p.turns_since_progress} 回合没有剧情进展</span>
          {p.repeating && <span className="warn-text">玩家在重复同样的行动</span>}
          {!ended && (
            <button
              disabled={busy}
              onClick={() => {
                setBusy(true);
                api
                  .directorAnalyze(ctx.worldId, campaignId)
                  .then(() => {
                    ctx.notify("导演给出了新的建议");
                    void load();
                  })
                  .catch(fail)
                  .finally(() => setBusy(false));
              }}
            >
              {busy ? "分析中…" : "请导演分析"}
            </button>
          )}
        </div>
        <div className="hint">{p.tension_reasons.join("；")}</div>
        {p.curve.length > 0 && (
          <div className="tension-curve" title="每回合的紧张度">
            {p.curve.map((b) => (
              <span key={b.turn} style={{ height: `${(b.tension / maxT) * 100}%` }} className={b.progress.length ? "progress" : ""} title={`第 ${b.turn} 回合：${b.tension}${b.progress.length ? "；" + b.progress.join("、") : ""}`} />
            ))}
          </div>
        )}
        {p.recent_progress.length > 0 && <div className="hint">最近的进展：{p.recent_progress.join("；")}</div>}
        {con.notes.length === 0 && <p className="hint">还没有导演建议。剧情停滞、玩家重复行动或每隔几回合，导演会在后台自动分析；也可以手动请求。</p>}
        {con.notes.slice(0, 4).map((n) => (
          <div key={n.id} className="director-note">
            <div>
              <span className="badge">{TRIGGER[n.trigger] ?? n.trigger}</span> 第 {n.turn} 回合 · <strong>{PACE[n.pacing] ?? n.pacing}</strong>：{n.assessment}
            </div>
            {n.conformity !== null && n.conformity !== undefined && (
              <div className="conformity" title="和原剧本、世界观、人物设定、已发生事件的一致程度（导演模型评估）">
                <span>剧情符合度</span>
                <span className="bar">
                  <span style={{ transform: `scaleX(${n.conformity / 100})` }} className={n.conformity >= 70 ? "ok" : n.conformity >= 40 ? "mid" : "low"} />
                </span>
                <strong>{n.conformity}</strong>
              </div>
            )}
            {n.deviations.length > 0 && (
              <ul className="deviations">
                {n.deviations.map((d, i) => (
                  <li key={i}>
                    <span className="badge">{DEV[d.kind] ?? d.kind}</span> {d.text}
                  </li>
                ))}
              </ul>
            )}
            <ul className="list">
              {n.suggestions.map((s) => (
                <li key={s.id}>
                  <span className="badge">{KIND[s.kind] ?? s.kind}</span> {s.text}
                  {s.needs_review && <span className="warn-text">（可能改动已确认设定，需要你确认）</span>}
                  <span className="hint">　{STATUS[s.status] ?? s.status}</span>
                  <div className="hint">
                    {s.why}
                    {s.uses.length > 0 && `；依据：${s.uses.join("；")}`}
                  </div>
                  {!ended && (
                    <span>
                      {s.status !== "adopted" && <button className="small" onClick={() => setStatus(n.id, s.id, "adopted")}>采用</button>}
                      {s.status === "adopted" && <button className="small" onClick={() => setStatus(n.id, s.id, "done")}>完成</button>}
                      {s.status !== "dismissed" && <button className="small link" onClick={() => setStatus(n.id, s.id, "dismissed")}>忽略</button>}
                    </span>
                  )}
                </li>
              ))}
            </ul>
          </div>
        ))}
        <p className="hint">采用的建议会作为主持人信息交给 GM，GM 只在合适时给玩家机会，不会强迫玩家；导演不修改任何数据。</p>
        {!ended &&
          (settings ? (
            <div className="rules">
              <label className="check">
                <input type="checkbox" checked={settings.auto_analyze} onChange={(e) => setSettings({ ...settings, auto_analyze: e.target.checked })} />
                自动分析
              </label>
              <label className="check">
                每 <input type="number" min={0} value={settings.every_turns} onChange={(e) => setSettings({ ...settings, every_turns: Number(e.target.value) })} style={{ width: 50 }} /> 回合例行分析（0 = 只在停滞时）
              </label>
              <label className="check">
                连续 <input type="number" min={1} value={settings.stall_turns} onChange={(e) => setSettings({ ...settings, stall_turns: Number(e.target.value) })} style={{ width: 50 }} /> 回合没有进展算停滞
              </label>
              <label className="check">
                <input type="checkbox" checked={settings.auto_adopt_on_stall} onChange={(e) => setSettings({ ...settings, auto_adopt_on_stall: e.target.checked })} />
                停滞时自动采用第一条建议
              </label>
              <label className="check pacing-slider" title="影响导演建议的节奏：松弛时给玩家更多自由探索，紧凑时事件更密集">
                节奏　松弛
                <input type="range" min={0} max={100} step={10} value={settings.pacing_level} onChange={(e) => setSettings({ ...settings, pacing_level: Number(e.target.value) })} />
                紧凑
              </label>
              <label className="check">
                <input type="checkbox" checked={settings.hint_only} onChange={(e) => setSettings({ ...settings, hint_only: e.target.checked })} />
                偏离剧本时只提示，不拉回主线（也不会自动采用建议）
              </label>
              <label className="check">
                采用的建议保留 <input type="number" min={1} value={settings.guidance_turns} onChange={(e) => setSettings({ ...settings, guidance_turns: Number(e.target.value) })} style={{ width: 50 }} /> 回合
              </label>
              <button className="primary" onClick={() => void api.directorSetSettings(ctx.worldId, campaignId, settings).then(() => { setSettings(null); void load(); }).catch(fail)}>保存</button>
              <button onClick={() => setSettings(null)}>取消</button>
            </div>
          ) : (
            <button className="small" onClick={() => setSettings(con.settings)}>导演设置</button>
          ))}
      </Section>

      <Section title={`因果与伏笔（${con.consequences.length + con.foreshadows.length}）`}>
        {con.consequences.length === 0 && con.foreshadows.length === 0 && <p className="hint">还没有记下的后果或伏笔。玩家做出重要决定时，GM 会记下后果（可以延迟发生）；已经发生的后果不能撤销。</p>}
        {con.consequences.length > 0 && (
          <ul className="list causal">
            {con.consequences.map((c) => (
              <li key={c.id}>
                <span className={`badge ${c.status === "occurred" ? "ok" : ""}`}>{c.status === "occurred" ? `第 ${c.occurred_turn} 回合已发生` : `第 ${c.due_turn} 回合到期`}</span> 「{c.decision}」→ {c.consequence}
                <small className="hint">（第 {c.created_turn} 回合的决定）</small>
              </li>
            ))}
          </ul>
        )}
        {con.foreshadows.length > 0 && (
          <ul className="list">
            {con.foreshadows.map((f) => (
              <li key={f.id}>
                <span className={`badge ${f.status === "paid" ? "ok" : ""}`}>{f.status === "paid" ? `第 ${f.paid_turn} 回合回收` : "未回收"}</span> {f.text}
                <small className="hint">（第 {f.planted_turn} 回合埋下{f.payoff ? `；回收：${f.payoff}` : ""}）</small>
              </li>
            ))}
          </ul>
        )}
      </Section>

      <Section title={`剧情冲突提示（${con.conflicts.length}）`}>
        {con.conflicts.length === 0 ? (
          <p className="hint">没有发现冲突。</p>
        ) : (
          <ul className="list">
            {con.conflicts.map((c, i) => (
              <li key={i} className={c.severity === "warn" ? "warn-text" : "hint"}>
                {c.turn !== null ? `第 ${c.turn} 回合：` : ""}
                {c.text}
              </li>
            ))}
          </ul>
        )}
      </Section>

      <Section title="当前场景">
        <div>
          <strong>{con.scene.title || "（自由沙盒）"}</strong> <span className="hint">{con.scene.kind}</span>　地点：{con.scene.location || "—"}
        </div>
        {con.scene.player_text && <div>{con.scene.player_text}</div>}
        {con.scene.gm_text && <div className="gm-text">GM 笔记：{con.scene.gm_text}</div>}
        <div className="hint">在场：{con.scene.present.join("、") || "无"}</div>
        {con.scene.edges.length > 0 && (
          <table className="grid-table">
            <tbody>
              {con.scene.edges.map((e, i) => (
                <tr key={i}>
                  <td>{e.label} → {e.to}</td>
                  <td className={e.availability === "满足" ? "ok-text" : "hint"}>{e.used ? "已走过" : e.availability}</td>
                  <td className="hint mono">{e.condition}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Section>

      <Section title="世界状态和时间">
        <div>
          {con.time} · {con.environment.season}季 · {con.environment.weather} · 玩家在 {con.location || "—"}
        </div>
        <table className="grid-table">
          <tbody>
            {Object.entries(con.vars).map(([k, v]) => (
              <tr key={k}>
                <td className="mono">{k}</td>
                <td className="mono">{v}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </Section>

      <Section title={`人物（${con.npcs.length}）`}>
        <ul className="list">
          {con.npcs.map((n) => (
            <li key={n.id}>
              <strong>{n.name}</strong> <span className="hint">{n.role}</span> {n.present && <span className="ok-text">在场</span>}
              {n.life_status !== "alive" && <span className="warn-text">（{n.life_status}）</span>}
              <div className="hint">
                此刻：{n.activity || "—"}
                {n.location ? `（${n.location}）` : ""}
                {n.max_hp > 0 && `；${n.level} 级，生命 ${n.hp}/${n.max_hp}`}
              </div>
              {n.goals.length > 0 && <div>目标：{n.goals.join("；")}</div>}
              {n.secrets.length > 0 && <div className="gm-text">秘密：{n.secrets.join("；")}</div>}
              {n.mood && (
                <div className="hint">
                  恐惧 {n.mood.fear} 愤怒 {n.mood.anger} 怀疑 {n.mood.suspicion} 压力 {n.mood.stress} 忠诚 {n.mood.loyalty}
                  {n.bond && `；对玩家：信任 ${n.bond.trust} 好感 ${n.bond.affection} 敌意 ${n.bond.hostility}`}
                </div>
              )}
            </li>
          ))}
        </ul>
      </Section>

      <Section title="隐藏剧情" open={false}>
        {con.hidden.core_setting && <div className="gm-text">核心设定与真相：{con.hidden.core_setting}</div>}
        {con.hidden.hidden_quests.length > 0 && <div>未开始的隐藏任务：{con.hidden.hidden_quests.join("、")}</div>}
        <div className="hint">还没到达的剧情节点：</div>
        <ul className="list">
          {con.hidden.upcoming.map(([t, g], i) => (
            <li key={i}>
              {t}
              {g && <span className="gm-text">　{g}</span>}
            </li>
          ))}
        </ul>
      </Section>

      <Section title="线索和触发条件" open={false}>
        <ul className="list">
          {con.clues.map((c) => (
            <li key={c.id}>
              <span className={c.revealed ? "ok-text" : "hint"}>{c.revealed ? "✔ 已揭示" : "○ 未揭示"}</span> {c.title}
              {c.gm_text && <span className="gm-text">　{c.gm_text}</span>}
              {!c.revealed && !ended && (
                <button
                  className="small"
                  onClick={() =>
                    void api
                      .consoleApply(ctx.worldId, campaignId, [{ type: "reveal_clue", clue: c.id }], `主持人揭示线索「${c.title}」`)
                      .then(() => {
                        ctx.notify(`已揭示「${c.title}」`);
                        void load();
                        onChanged();
                      })
                      .catch(fail)
                  }
                >
                  揭示
                </button>
              )}
            </li>
          ))}
        </ul>
        <table className="grid-table">
          <tbody>
            {con.conditions.map((c, i) => (
              <tr key={i}>
                <td>
                  {c.owner}
                  <div className="hint">{c.role}{c.fired ? "（已发生）" : ""}</div>
                </td>
                <td className={c.now === "满足" ? "ok-text" : "hint"}>{c.now}</td>
                <td className="hint mono">{c.condition}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </Section>

      <Section title="AI 决策与记忆检索记录">
        {con.traces.length === 0 && <p className="hint">还没有 GM 回合。</p>}
        <ul className="list">
          {con.traces.map((t) => {
            const tr = (t.context ?? null) as ContextTrace | null;
            return (
              <li key={t.turn}>
                <button className="link" onClick={() => setOpenTurn(openTurn === t.turn ? null : t.turn)}>
                  第 {t.turn} 回合：{t.input || "—"}
                </button>
                <span className="hint">　{t.intent}</span>
                {t.conflicts.length > 0 && <span className="warn-text">　⚠ {t.conflicts.length}</span>}
                {openTurn === t.turn && (
                  <div className="trace">
                    <div>决定依据：{t.based_on.length ? t.based_on.join("；") : "（GM 没有注明）"}</div>
                    {tr && (
                      <>
                        <div className="hint">
                          上下文 {tr.tokens}/{tr.budget} tokens：
                          {tr.sections.map((s) => `${s.title.split("（")[0]}[${SEC[s.status] ?? s.status}]`).join("、")}
                        </div>
                        <div>检索到的记忆：{tr.memories.length ? tr.memories.join("；") : "无"}</div>
                        <div>检索到的设定：{tr.search_hits.length ? tr.search_hits.join("；") : "无"}</div>
                        {tr.guidance.length > 0 && <div>采用中的导演建议：{tr.guidance.join("；")}</div>}
                      </>
                    )}
                    {t.npc_memories.map(([n, m], i) => (
                      <div key={i}>
                        {n} 的记忆：{m.length ? m.join("；") : "无"}
                      </div>
                    ))}
                    {t.rejected.length > 0 && <div className="warn-text">被拒绝的修改：{t.rejected.join("；")}</div>}
                    {t.conflicts.length > 0 && <div className="warn-text">冲突：{t.conflicts.join("；")}</div>}
                    <div className="hint">叙述：{t.narration}</div>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      </Section>

      <Section title="手动修改游戏状态" open={false}>
        {ended ? (
          <p className="hint">这条时间线已经结束。</p>
        ) : (
          <>
            <EffectsEditor value={effects} onChange={setEffects} choices={choices} />
            <div className="row">
              <input placeholder="原因（必填，会记录在事件日志里）" value={reason} onChange={(e) => setReason(e.target.value)} style={{ flex: 1 }} />
              <button
                className="primary"
                disabled={!effects.length || !reason.trim()}
                onClick={() =>
                  void api
                    .consoleApply(ctx.worldId, campaignId, effects, reason)
                    .then((n) => {
                      ctx.notify(`已执行 ${n} 项修改`);
                      setEffects([]);
                      setReason("");
                      void load();
                      onChanged();
                    })
                    .catch(fail)
                }
              >
                执行
              </button>
            </div>
          </>
        )}
        {con.manual_edits.length > 0 && (
          <ul className="list">
            {con.manual_edits.map((m, i) => (
              <li key={i}>
                第 {m.turn} 回合：{m.reason}
                <div className="hint mono">{m.effects.join("  ")}</div>
              </li>
            ))}
          </ul>
        )}
      </Section>
    </div>
  );
}
