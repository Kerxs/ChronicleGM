import RefImages from "../components/RefImages";
import { useEffect, useState } from "react";
import { api, type WorldOverview } from "../api";
import type { Ctx } from "../App";
import type { Script } from "../bindings/Script";
import type { DeletedScript } from "../bindings/DeletedScript";

export default function ScriptsPage({ ctx, overview }: { ctx: Ctx; overview: WorldOverview | null }) {
  const [scripts, setScripts] = useState<Script[]>([]);
  const [idea, setIdea] = useState("");
  const [title, setTitle] = useState("");
  const [genre, setGenre] = useState("");
  const [tone, setTone] = useState("");
  const [players, setPlayers] = useState(4);
  const [hours, setHours] = useState(4);
  const [rules, setRules] = useState("");
  const [useWorld, setUseWorld] = useState(true);
  const [busy, setBusy] = useState(false);
  const [refs, setRefs] = useState<string[]>([]);

  const [trash, setTrash] = useState<DeletedScript[]>([]);
  const [showTrash, setShowTrash] = useState(false);
  useEffect(() => {
    api.listScripts(ctx.worldId).then(setScripts).catch((e: Error) => ctx.notify(e.message, "error"));
    api.scriptTrash(ctx.worldId).then(setTrash).catch(() => setTrash([]));
  }, [ctx.worldId, ctx.tick]);

  async function act(p: Promise<unknown>, ok: string) {
    try {
      await p;
      ctx.notify(ok);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  async function generate() {
    setBusy(true);
    try {
      await api.startGeneration(ctx.worldId, {
        task: "outline",
        idea,
        title: title.trim() || null,
        genre,
        tone,
        player_count: players,
        duration_hours: hours,
        rules,
        use_world: useWorld,
      }, refs);
      ctx.notify("已开始生成剧本大纲，完成后会在审阅台出现");
      ctx.go({ name: "jobs" });
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    } finally {
      setBusy(false);
    }
  }

  async function createEmpty() {
    const t = title.trim() || "未命名剧本";
    try {
      const s = await api.createScript(ctx.worldId, {
        title: t,
        genre,
        premise: idea,
        background: "",
        core_setting: "",
        player_count: players,
        duration_hours: hours,
        tone,
        game_profile: {},
      });
      ctx.go({ name: "script", id: s.id });
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  return (
    <section>
      <h2>{overview?.info.name ?? "剧本"}</h2>
      <div className="grid2">
        <div className="card">
          <h3>剧本列表</h3>
          {scripts.length === 0 && <p className="hint">还没有剧本。</p>}
          <ul className="list">
            {scripts.map((s) => (
              <li key={s.id} className="script-row">
                <button onClick={() => ctx.go({ name: "script", id: s.id })}>
                  <strong>{s.title}</strong>
                  <small>
                    {s.genre || "未分类"} · {s.player_count} 人 · {s.duration_hours} 小时
                  </small>
                </button>
                <span className="row-actions">
                  <button className="small" title="连同条目、角色、剧情图一起复制" onClick={() => void act(api.copyScript(ctx.worldId, s.id, ""), "已复制「" + s.title + "」")}>
                    复制
                  </button>
                  <button
                    className="small danger"
                    title="连同条目、角色、剧情图一起移到回收站，可以恢复"
                    onClick={() => {
                      if (window.confirm("把剧本「" + s.title + "」移到回收站？\n条目、角色、剧情图会一起移走，可以在回收站恢复。删除前会自动保存一个版本。"))
                        void act(api.deleteScript(ctx.worldId, s.id), "已移到回收站");
                    }}
                  >
                    删除
                  </button>
                </span>
              </li>
            ))}
          </ul>
          {trash.length > 0 && (
            <div className="trash">
              <button className="link" onClick={() => setShowTrash(!showTrash)}>
                回收站（{trash.length}）{showTrash ? "▲" : "▼"}
              </button>
              {showTrash && (
                <ul className="list">
                  {trash.map((t) => (
                    <li key={t.id} className="script-row">
                      <span>
                        {t.title} <small className="hint">{t.items} 个条目 · {new Date(t.deleted_at).toLocaleString()}</small>
                      </span>
                      <button className="small" onClick={() => void act(api.restoreScript(ctx.worldId, t.id), "已恢复「" + t.title + "」")}>
                        恢复
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </div>
        <form
          className="card form"
          onSubmit={(e) => {
            e.preventDefault();
            void generate();
          }}
        >
          <h3>AI 生成新剧本</h3>
          <label>
            创意 *
            <textarea value={idea} onChange={(e) => setIdea(e.target.value)} rows={3} placeholder="例如：雾港的货船在一夜之间消失了，只留下一盏还亮着的船灯" />
          </label>
          <div className="row">
            <label>
              名称（可选）
              <input value={title} onChange={(e) => setTitle(e.target.value)} />
            </label>
            <label>
              题材
              <input value={genre} onChange={(e) => setGenre(e.target.value)} placeholder="悬疑 / 奇幻 / 克苏鲁…" />
            </label>
          </div>
          <div className="row">
            <label>
              风格基调
              <input value={tone} onChange={(e) => setTone(e.target.value)} placeholder="阴郁 / 轻松…" />
            </label>
            <label>
              玩家人数
              <input type="number" min={1} max={50} value={players} onChange={(e) => setPlayers(Number(e.target.value))} />
            </label>
            <label>
              时长（小时）
              <input type="number" min={0.5} max={500} step={0.5} value={hours} onChange={(e) => setHours(Number(e.target.value))} />
            </label>
          </div>
          <label>
            额外要求
            <input value={rules} onChange={(e) => setRules(e.target.value)} placeholder="例如：适合新手；不要出现血腥描写" />
          </label>
          <label className="check">
            <input type="checkbox" checked={useWorld} onChange={(e) => setUseWorld(e.target.checked)} />
            参考当前世界的设定与已有人物
          </label>
          <RefImages ctx={ctx} value={refs} onChange={setRefs} />
          <p className="hint">分段生成：先生成大纲，确认写入后再逐章展开。生成结果需要在审阅台确认后才会写入世界。</p>
          <div className="row">
            <button type="submit" className="primary" disabled={busy || !idea.trim()}>
              生成大纲
            </button>
            <button type="button" onClick={() => void createEmpty()}>
              手动新建空剧本
            </button>
          </div>
        </form>
      </div>
    </section>
  );
}
