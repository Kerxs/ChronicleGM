import { useEffect, useMemo, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { api, formatTime } from "../api";
import type { Ctx } from "../App";
import type { ArchiveEntry } from "../bindings/ArchiveEntry";
import type { CampaignStats } from "../bindings/CampaignStats";
import type { EndingDistribution } from "../bindings/EndingDistribution";
import type { Chapter } from "../bindings/Chapter";
import type { Chronicle } from "../bindings/Chronicle";
import type { EventHit } from "../bindings/EventHit";
import type { CampaignCheck } from "../bindings/CampaignCheck";

const QUEST: Record<string, string> = { "": "未开始", active: "进行中", done: "已完成", failed: "已失败", refused: "已拒绝", bypassed: "已绕过", invalid: "已失效", changed: "被玩家改变" };
const COMBAT: Record<string, string> = { victory: "胜利", defeat: "失败", fled: "撤退", ended: "中止", active: "进行中" };
const KIND: Record<string, string> = {
  player: "玩家行动",
  narration: "叙述",
  npc: "NPC 发言",
  fact: "事实",
  chapter: "章节",
  campaign_end: "结局",
  summary: "会话摘要",
  effects: "状态变化",
  transition: "剧情跳转",
};
const ENTRY: Record<string, string> = { action: "行动", scene: "场景", progress: "进展", death: "死亡", bond: "关系", combat: "战斗", ending: "结局", fork: "回溯", note: "叙述" };

function duration(ms: number): string {
  const m = Math.round(ms / 60000);
  return m < 60 ? `${m} 分钟` : `${Math.floor(m / 60)} 小时 ${m % 60} 分钟`;
}

export default function Archive({ ctx, onOpenTools }: { ctx: Ctx; onOpenTools: () => void }) {
  const [list, setList] = useState<ArchiveEntry[]>([]);
  const [sel, setSel] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [status, setStatus] = useState<"all" | "active" | "ended" | "archived">("all");
  const [tab, setTab] = useState<"stats" | "chronicle" | "chapters" | "search" | "check">("stats");
  const [stats, setStats] = useState<CampaignStats | null>(null);
  const [endings, setEndings] = useState<EndingDistribution | null>(null);
  const [chron, setChron] = useState<Chronicle | null>(null);
  const [chapters, setChapters] = useState<Chapter[]>([]);
  const [newChapter, setNewChapter] = useState("");
  const [query, setQuery] = useState("");
  const [kinds, setKinds] = useState<string[]>([]);
  const [onlySel, setOnlySel] = useState(false);
  const [hits, setHits] = useState<EventHit[] | null>(null);
  const [checks, setChecks] = useState<CampaignCheck[] | null>(null);
  const [busy, setBusy] = useState(false);
  const fail = (e: Error) => ctx.notify(e.message, "error");

  const reload = () => api.archiveList(ctx.worldId).then(setList).catch(fail);
  useEffect(() => {
    void reload();
  }, [ctx.worldId, ctx.tick]);

  const cur = list.find((e) => e.campaign.id === sel) ?? null;
  useEffect(() => {
    setStats(null);
    setChron(null);
    setEndings(null);
    if (!sel) return;
    api.archiveStats(ctx.worldId, sel).then(setStats).catch(fail);
    api.archiveChapters(ctx.worldId, sel).then(setChapters).catch(fail);
  }, [sel, ctx.worldId, ctx.tick]);
  useEffect(() => {
    if (cur?.campaign.script_id) api.archiveEndings(ctx.worldId, cur.campaign.script_id).then(setEndings).catch(fail);
  }, [cur?.campaign.script_id, ctx.worldId, ctx.tick]);
  useEffect(() => {
    if (sel && tab === "chronicle" && !chron) api.archiveChronicle(ctx.worldId, sel).then(setChron).catch(fail);
  }, [sel, tab, chron]);

  const shown = useMemo(
    () =>
      list.filter((e) => {
        const c = e.campaign;
        if (status === "archived" ? !c.archived : status !== "all" && (c.archived || c.status !== status)) return false;
        const f = filter.trim();
        return !f || c.name.includes(f) || e.script_title.includes(f) || c.ending.includes(f);
      }),
    [list, filter, status],
  );

  async function exportJson(withDesc: boolean) {
    if (!cur) return;
    const path = await save({ defaultPath: `${cur.campaign.name}.campaign.json`, filters: [{ name: "战役 JSON", extensions: ["json"] }] });
    if (!path) return;
    try {
      const n = await api.archiveExport(ctx.worldId, cur.campaign.id, path, withDesc);
      ctx.notify(`已导出 ${n} 条时间线到 ${path}`);
    } catch (e) {
      fail(e as Error);
    }
  }

  async function importJson() {
    const path = await open({ multiple: false, filters: [{ name: "战役 JSON", extensions: ["json"] }] });
    if (!path || Array.isArray(path)) return;
    setBusy(true);
    try {
      const r = await api.archiveImport(ctx.worldId, path);
      ctx.notify(`已导入 ${r.names.join("、")}（${r.events} 条事件，已先自动备份）${r.warnings.length ? "。注意：" + r.warnings.join("；") : ""}`);
      ctx.bump();
      setSel(r.campaign_ids[0] ?? null);
    } catch (e) {
      fail(e as Error);
    } finally {
      setBusy(false);
    }
  }

  async function saveChronicle() {
    if (!cur) return;
    const path = await save({ defaultPath: `${cur.campaign.name} 编年史.md`, filters: [{ name: "Markdown", extensions: ["md"] }] });
    if (!path) return;
    api.archiveSaveChronicle(ctx.worldId, cur.campaign.id, path).then(() => ctx.notify(`已保存到 ${path}`)).catch(fail);
  }

  async function search() {
    const t0 = performance.now();
    try {
      const r = await api.archiveSearch(ctx.worldId, { query, kinds, campaign_ids: onlySel && sel ? [sel] : [], limit: 300 });
      setHits(r);
      ctx.notify(`找到 ${r.length} 条（${Math.round(performance.now() - t0)} ms）`);
    } catch (e) {
      fail(e as Error);
    }
  }

  async function check() {
    setBusy(true);
    try {
      setChecks(await api.archiveCheck(ctx.worldId));
      setTab("check");
    } catch (e) {
      fail(e as Error);
    } finally {
      setBusy(false);
    }
  }

  const refreshChapters = () => {
    if (sel) {
      api.archiveChapters(ctx.worldId, sel).then(setChapters).catch(fail);
      setChron(null);
    }
  };

  return (
    <div className="page archive">
      <header className="row">
        <h2>战役档案馆</h2>
        <span className="spacer" />
        <button disabled={busy} onClick={() => void importJson()}>导入战役 JSON</button>
        <button disabled={busy} onClick={() => void check()}>检查所有战役</button>
        <button onClick={onOpenTools}>备份与还原…</button>
      </header>
      <div className="grid2">
        <section className="card">
          <div className="row">
            <input value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="按名称、剧本、结局筛选" />
            <select value={status} onChange={(e) => setStatus(e.target.value as typeof status)}>
              <option value="all">全部（不含已归档）</option>
              <option value="active">进行中</option>
              <option value="ended">已结束</option>
              <option value="archived">已归档</option>
            </select>
          </div>
          {shown.length === 0 && <p className="hint">没有战役。</p>}
          <ul className="list">
            {shown
              .filter((e) => status === "archived" || !e.campaign.archived)
              .map((e) => (
                <li key={e.campaign.id} style={{ paddingLeft: e.depth * 18 }}>
                  <button className={sel === e.campaign.id ? "active" : ""} onClick={() => setSel(e.campaign.id)}>
                    {e.depth > 0 && "↳ "}
                    <b>{e.campaign.name}</b>
                    <small>
                      {" "}
                      {e.script_title || "无剧本"} · 第 {e.campaign.turn} 回合 · {e.events} 条事件 · {e.campaign.status === "ended" ? `已结束：${e.campaign.ending}` : "进行中"}
                      {e.children > 0 && ` · ${e.children} 条分支`} · {formatTime(e.last_activity)}
                    </small>
                  </button>
                </li>
              ))}
          </ul>
        </section>
        <section>
          <div className="tabs">
            {(
              [
                ["stats", "统计"],
                ["chronicle", "编年史"],
                ["chapters", "章节"],
                ["search", "搜索"],
                ["check", "完整性"],
              ] as const
            ).map(([k, l]) => (
              <button key={k} className={tab === k ? "active" : ""} onClick={() => setTab(k)}>
                {l}
              </button>
            ))}
          </div>
          {cur && tab !== "search" && tab !== "check" && (
            <div className="row card">
              <b>{cur.campaign.name}</b>
              <span className="spacer" />
              <button onClick={() => void exportJson(false)}>导出 JSON</button>
              {cur.children > 0 && <button onClick={() => void exportJson(true)}>连同分支导出</button>}
              <button
                onClick={() =>
                  api
                    .timelineArchive(ctx.worldId, cur.campaign.id, !cur.campaign.archived)
                    .then(() => reload())
                    .catch(fail)
                }
              >
                {cur.campaign.archived ? "取消归档" : "归档"}
              </button>
            </div>
          )}
          {!cur && tab !== "search" && tab !== "check" && <p className="hint">在左边选择一个战役。</p>}

          {tab === "stats" && stats && (
            <>
              <dl className="stats">
                <div><dt>回合</dt><dd>{stats.turns}</dd></div>
                <div><dt>玩家行动</dt><dd>{stats.player_actions}</dd></div>
                <div><dt>掷骰</dt><dd>{stats.rolls}</dd></div>
                <div><dt>游玩时长</dt><dd style={{ fontSize: 16 }}>{duration(stats.play_ms)}</dd></div>
                <div><dt>NPC 存活 / 死亡</dt><dd>{stats.npcs_alive} / {stats.npcs_dead}</dd></div>
                <div><dt>关系变化</dt><dd>{stats.bond_changes}</dd></div>
                <div><dt>线索</dt><dd>{stats.clues_revealed} / {stats.clues_total}</dd></div>
                <div><dt>剧情节点到过</dt><dd>{stats.plot_visited} / {stats.plot_total}</dd></div>
              </dl>
              <div className="card">
                <h3>玩家选择与剧情分支</h3>
                {stats.choices.length === 0 ? <p className="hint">还没有剧情跳转。</p> : (
                  <table>
                    <thead><tr><th>回合</th><th>从</th><th>选择</th><th>到</th><th>分支</th></tr></thead>
                    <tbody>
                      {stats.choices.map((c) => (
                        <tr key={c.seq}>
                          <td>{c.turn}</td><td>{c.from}</td><td>{c.label || "（无标签）"}{!c.passed && "（检定未通过）"}</td><td>{c.to}</td><td>{c.options > 1 ? `${c.options} 选 1` : ""}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
                <p className="hint">剧本共有 {stats.branch_points} 个分支点、{stats.endings_total} 个结局；跳转失败 {stats.transitions_failed} 次。</p>
              </div>
              <div className="card">
                <h3>任务与战斗</h3>
                <p>任务：{Object.entries(stats.quests).map(([k, n]) => `${QUEST[k] ?? k} ${n}`).join(" · ") || "无"}</p>
                <p>战斗：{Object.entries(stats.combat_results).map(([k, n]) => `${COMBAT[k] ?? k} ${n}`).join(" · ") || "无"}</p>
                {stats.combats.map((c) => (
                  <p key={c.id} className="hint">{c.reason || "战斗"}：{COMBAT[c.status] ?? c.status}，{c.rounds} 轮，{c.enemies} 个敌人{c.rewards.length > 0 && `，${c.rewards.join("、")}`}</p>
                ))}
              </div>
              <div className="card">
                <h3>NPC 与关系</h3>
                <p>{stats.npcs.map((n) => `${n.name}${n.life_status === "dead" ? `（死亡${n.died_turn != null ? `，第 ${n.died_turn} 回合` : ""}）` : ""}`).join("、") || "无"}</p>
                {stats.bonds.slice(0, 12).map((b, i) => (
                  <p key={i} className="hint">
                    {b.from} → {b.to}：{b.first_kind || "无"} → {b.last_kind || "未分类"}，{b.changes} 次变化（强度 {b.delta[0]}，信任 {b.delta[1]}，好感 {b.delta[2]}，敌意 {b.delta[3]}）
                  </p>
                ))}
              </div>
              {endings && (
                <div className="card">
                  <h3>结局分布（同剧本的 {endings.total} 条时间线）</h3>
                  <p>已结束 {endings.ended} · 进行中 {endings.active}</p>
                  {endings.endings.map((e) => (
                    <p key={e.ending}>
                      <b>{e.ending}</b> × {e.count}
                      <small className="hint">（{e.campaigns.join("、")}）</small>
                    </p>
                  ))}
                  {endings.unreached.length > 0 && <p className="hint">还没有达到：{endings.unreached.join("、")}</p>}
                </div>
              )}
              {(stats.parent || stats.children > 0) && (
                <p className="hint">时间线：{stats.parent ? `从「${stats.parent}」回溯而来` : "根时间线"}{stats.children > 0 && `，回溯出 ${stats.children} 条分支`}</p>
              )}
            </>
          )}

          {tab === "chronicle" && cur && (
            <div className="card">
              <div className="row">
                <span className="hint">按事件日志自动生成（不调用模型，存档不变结果就不变）。</span>
                <span className="spacer" />
                <button onClick={() => void saveChronicle()}>保存为 Markdown</button>
              </div>
              {!chron && <p className="hint">生成中…</p>}
              {chron?.chapters.map((c) => (
                <div key={c.chapter.no}>
                  <h3>第 {c.chapter.no} 章 {c.chapter.title}<small className="hint">（回合 {c.chapter.turn_from}–{c.chapter.turn_to}）</small></h3>
                  {c.chapter.summary && <blockquote>{c.chapter.summary}</blockquote>}
                  <ul>
                    {c.entries.filter((e) => e.kind !== "note").map((e, i) => (
                      <li key={i}><small className="hint">[{e.turn}] {ENTRY[e.kind] ?? e.kind}</small> {e.text}</li>
                    ))}
                  </ul>
                </div>
              ))}
            </div>
          )}

          {tab === "chapters" && cur && (
            <div className="card">
              <form className="row" onSubmit={(e) => {
                e.preventDefault();
                api.archiveMarkChapter(ctx.worldId, cur.campaign.id, newChapter).then(() => { setNewChapter(""); refreshChapters(); }).catch(fail);
              }}>
                <input value={newChapter} onChange={(e) => setNewChapter(e.target.value)} placeholder="在当前位置开始新的一章：章节标题" maxLength={80} />
                <button type="submit" disabled={!newChapter.trim()}>开始新章节</button>
              </form>
              <p className="hint">没有手动章节时，按会话摘要分章；也没有摘要时，按进入的剧情节点分章。章节标记只追加记录，不改变战役状态。</p>
              <table>
                <thead><tr><th>#</th><th>标题</th><th>回合</th><th>来源</th><th></th></tr></thead>
                <tbody>
                  {chapters.map((c) => (
                    <tr key={c.seq_from}>
                      <td>{c.no}</td>
                      <td>{c.title}</td>
                      <td>{c.turn_from}–{c.turn_to}</td>
                      <td>{{ manual: "手动", summary: "会话摘要", scene: "剧情节点", all: "全程" }[c.source] ?? c.source}</td>
                      <td>
                        {c.source === "manual" && (
                          <>
                            <button className="link" onClick={() => {
                              const t = window.prompt("新的章节标题", c.title);
                              if (t) api.archiveRenameChapter(ctx.worldId, cur.campaign.id, c.seq_from, t).then(refreshChapters).catch(fail);
                            }}>改名</button>
                            <button className="link danger" onClick={() => api.archiveRemoveChapter(ctx.worldId, cur.campaign.id, c.seq_from).then(refreshChapters).catch(fail)}>删除标记</button>
                          </>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          {tab === "search" && (
            <div className="card">
              <form className="row" onSubmit={(e) => { e.preventDefault(); void search(); }}>
                <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="搜索所有战役的行动、叙述、NPC 发言、摘要…" />
                <button type="submit" className="primary" disabled={!query.trim()}>搜索</button>
              </form>
              <div className="row">
                {["player", "narration", "npc", "fact", "summary"].map((k) => (
                  <label key={k}>
                    <input type="checkbox" checked={kinds.includes(k)} onChange={(e) => setKinds(e.target.checked ? [...kinds, k] : kinds.filter((x) => x !== k))} /> {KIND[k]}
                  </label>
                ))}
                <label><input type="checkbox" checked={onlySel} disabled={!sel} onChange={(e) => setOnlySel(e.target.checked)} /> 只搜当前选中的战役</label>
              </div>
              {hits && hits.length === 0 && <p className="hint">没有结果。</p>}
              <ul className="list">
                {hits?.map((h, i) => (
                  <li key={i}>
                    <button onClick={() => setSel(h.campaign_id)}>
                      <small className="hint">{h.campaign_name} · {h.kind === "summary" ? `第 ${h.turn} 次游戏` : `回合 ${h.turn}`} · {KIND[h.kind] ?? h.kind}</small>
                      <div>{h.snippet}</div>
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          )}

          {tab === "check" && (
            <div className="card">
              {!checks && <p className="hint">点「检查所有战役」：逐条时间线重放事件并与检查点比对，并检查剧本、剧情节点、父时间线是否存在。</p>}
              {checks && (
                <p>{checks.filter((c) => c.ok).length} / {checks.length} 条时间线通过。</p>
              )}
              {checks?.map((c) => (
                <div key={c.campaign_id} className={c.ok ? "" : "error"}>
                  <b>{c.ok ? "✓" : "✗"} {c.name}</b> <small className="hint">{c.checkpoints} 个检查点 · {c.events} 条事件</small>
                  {c.problems.map((p, i) => <p key={i} className="error">{p}</p>)}
                  {c.notes.map((p, i) => <p key={i} className="hint">{p}</p>)}
                </div>
              ))}
              {checks && checks.some((c) => !c.ok) && (
                <p className="hint">有问题的战役可以到「备份与还原」从备份恢复整个世界（恢复前会先另存当前状态）。</p>
              )}
            </div>
          )}
        </section>
      </div>
    </div>
  );
}
