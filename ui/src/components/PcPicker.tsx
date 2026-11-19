// 新战役第一步：选择剧本里的可选角色，或让 AI 按剧本生成一个新角色；看身世，确认后开始开场回合。
import { useEffect, useState } from "react";
import { api } from "../api";
import type { Ctx } from "../App";
import type { PlayerCharacter } from "../bindings/PlayerCharacter";

const BS: [string, string][] = [
  ["origin", "出身"],
  ["upbringing", "成长经历"],
  ["occupation", "职业"],
  ["key_events", "重要经历"],
  ["relations", "初始关系"],
];

function Backstory({ pc }: { pc: PlayerCharacter }) {
  const b = (pc.backstory ?? {}) as Record<string, unknown>;
  const rows = BS.map(([k, label]) => {
    const v = b[k];
    const text = Array.isArray(v) ? v.filter(Boolean).join("；") : typeof v === "string" ? v : "";
    return text ? (
      <div key={k} className="bs-row">
        <span className="bs-k">{label}</span>
        <span>{text}</span>
      </div>
    ) : null;
  }).filter(Boolean);
  if (rows.length === 0) return <p className="hint">这个角色还没有身世。</p>;
  return <div className="backstory">{rows}</div>;
}

export default function PcPicker({ ctx, scriptId, campaignId, onDone }: { ctx: Ctx; scriptId: string | null; campaignId: string; onDone: () => void }) {
  const [list, setList] = useState<PlayerCharacter[] | null>(null);
  const [picked, setPicked] = useState(0);
  const [hint, setHint] = useState("");
  const [busy, setBusy] = useState<"gen" | "set" | null>(null);
  useEffect(() => {
    if (!scriptId) {
      setList([]);
      return;
    }
    api
      .scriptPlayerCharacters(ctx.worldId, scriptId)
      .then(setList)
      .catch(() => setList([]));
  }, [ctx.worldId, scriptId]);

  async function generate() {
    if (!scriptId) return;
    setBusy("gen");
    try {
      const pc = await api.generatePc(ctx.worldId, scriptId, hint);
      setList((l) => [...(l ?? []), pc]);
      setPicked(list?.length ?? 0);
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    } finally {
      setBusy(null);
    }
  }

  async function confirm(pc: PlayerCharacter | null) {
    setBusy("set");
    try {
      if (pc) await api.campaignSetPc(ctx.worldId, campaignId, pc);
      onDone();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    } finally {
      setBusy(null);
    }
  }

  if (list === null) return null;
  const cur = list[picked];
  return (
    <div className="pc-picker card" data-tour="pc-picker">
      <h3>选择你的角色</h3>
      {list.length === 0 && <p className="hint">{scriptId ? "这个剧本还没有可选角色，可以让 AI 按剧本生成一个。" : "自由沙盒没有预设角色，可以直接开始。"}</p>}
      {list.length > 0 && (
        <div className="pc-cards">
          {list.map((pc, i) => (
            <button key={`${pc.name}-${i}`} type="button" className={`pc-card${i === picked ? " on" : ""}`} style={{ animationDelay: `${i * 0.05}s` }} onClick={() => setPicked(i)}>
              <strong>{pc.name}</strong>
              <span className="hint">{pc.identity}</span>
              {!pc.source_item && <span className="badge">AI 新角色</span>}
            </button>
          ))}
        </div>
      )}
      {cur && (
        <div className="pc-detail" key={picked}>
          {cur.intro && <p>{cur.intro}</p>}
          <div className="bs-row">
            <span className="bs-k">性格</span>
            <span>{cur.personality || "—"}</span>
          </div>
          <div className="bs-row">
            <span className="bs-k">擅长</span>
            <span>{cur.abilities.join("、") || "—"}</span>
          </div>
          <Backstory pc={cur} />
          <p className="hint">属性按剧本的属性方案和角色擅长的事自动分配，开始后可以在「角色」页查看和修改。</p>
        </div>
      )}
      {scriptId && (
        <div className="row pc-gen">
          <input value={hint} onChange={(e) => setHint(e.target.value)} placeholder="想要什么样的新角色？（可选）" title="例如：一个怕水的船医" disabled={busy !== null} />
          <button type="button" disabled={busy !== null} onClick={() => void generate()}>
            {busy === "gen" ? "AI 正在设计角色…" : "让 AI 生成新角色"}
          </button>
        </div>
      )}
      <div className="row pc-confirm-row">
        <button type="button" className="primary" data-tour="pc-confirm" disabled={busy !== null || (list.length > 0 && !cur)} onClick={() => void confirm(cur ?? null)}>
          {cur ? `扮演「${cur.name}」，开始游戏` : "直接开始游戏"}
        </button>
      </div>
    </div>
  );
}
