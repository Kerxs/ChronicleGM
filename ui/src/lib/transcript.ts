// 把战役事件日志转换成对话记录（纯函数）。主持人专用的条目带 gmOnly 标记，玩家视角下隐藏。
import type { EventEntry } from "../bindings/EventEntry";

export interface Line {
  seq: number;
  turn: number;
  who: "player" | "gm" | "npc" | "system";
  name?: string;
  /** NPC 台词的人物 ID（朗读时用它的声音）。 */
  speaker?: string;
  text: string;
  gmOnly: boolean;
  warn?: boolean;
}

type P = Record<string, unknown>;
const s = (v: unknown) => (typeof v === "string" ? v : "");

export function toTranscript(events: EventEntry[]): Line[] {
  const out: Line[] = [];
  for (const e of events) {
    const p = (e.payload ?? {}) as P;
    const base = { seq: e.seq, turn: e.turn };
    switch (e.kind) {
      case "player":
        out.push({ ...base, who: "player", text: s(p.text), gmOnly: false });
        break;
      case "narration": {
        if (s(p.text).trim()) out.push({ ...base, who: "gm", text: s(p.text), gmOnly: false });
        const leaks = Array.isArray(p.leaks) ? (p.leaks as string[]) : [];
        if (leaks.length) out.push({ ...base, who: "system", text: `⚠ 叙述中原样出现了主持人信息：${leaks.join("；")}`, gmOnly: true, warn: true });
        break;
      }
      case "npc": {
        out.push({ ...base, who: "npc", name: s(p.name), speaker: s(p.npc_id), text: s(p.text), gmOnly: false });
        const leaks = Array.isArray(p.leaks) ? (p.leaks as string[]) : [];
        if (leaks.length) out.push({ ...base, who: "system", text: `⚠ ${s(p.name)} 说出了不该知道的信息：${leaks.join("；")}`, gmOnly: true, warn: true });
        break;
      }
      case "roll": {
        const r = (p.result ?? {}) as P;
        out.push({ ...base, who: "system", text: `🎲 ${s(p.reason)}：${s(r.expr)} = ${String(r.total ?? "")}`, gmOnly: false });
        break;
      }
      case "transition":
        out.push({ ...base, who: "system", text: "➜ 剧情推进", gmOnly: false });
        break;
      case "transition_failed":
        out.push({ ...base, who: "system", text: "✗ 尝试推进剧情，但条件没有满足", gmOnly: false });
        break;
      case "effects_rejected": {
        const list = Array.isArray(p.rejected) ? (p.rejected as P[]) : [];
        out.push({ ...base, who: "system", text: `程序拒绝了 ${list.length} 条 AI 提议的状态修改：${list.map((x) => s(x.reason)).join("；")}`, gmOnly: true, warn: true });
        break;
      }
      case "speech_redirected": {
        const npcs = Array.isArray(p.npcs) ? (p.npcs as string[]) : [];
        out.push({ ...base, who: "system", text: `GM 在叙述里替 ${npcs.join("、")} 说了话，程序已删掉这些台词，改由人物亲自回答`, gmOnly: true });
        break;
      }
      case "fact":
        out.push({ ...base, who: "system", text: `📌 记住：${s(p.text)}`, gmOnly: true });
        break;
      case "gm_plan":
        if (s(p.intent)) out.push({ ...base, who: "system", text: `GM 判断玩家意图：${s(p.intent)}${p.off_script ? "（剧本外行动）" : ""}`, gmOnly: true });
        break;
      default:
        break;
    }
  }
  return out;
}
