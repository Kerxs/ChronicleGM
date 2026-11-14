import { describe, expect, it } from "vitest";
import { toTranscript } from "./transcript";
import type { EventEntry } from "../bindings/EventEntry";

const ev = (seq: number, kind: string, payload: unknown): EventEntry => ({ seq, turn: 1, kind, payload: payload as never, created_at: 0 });

describe("toTranscript", () => {
  it("区分玩家可见与主持人专用条目", () => {
    const lines = toTranscript([
      ev(1, "player", { text: "我推开门" }),
      ev(2, "gm_plan", { intent: "进入房间", off_script: false }),
      ev(3, "roll", { reason: "察觉 检定", result: { expr: "1d20", total: 14 } }),
      ev(4, "effects_rejected", { rejected: [{ reason: "变量 skill.x 由程序管理" }] }),
      ev(5, "narration", { text: "门后一片漆黑。", leaks: ["祭坛封印着古神"] }),
      ev(6, "npc", { name: "村长", text: "小心。", leaks: [] }),
      ev(7, "effects", { changes: [] }),
    ]);
    expect(lines.map((l) => l.who)).toEqual(["player", "system", "system", "system", "gm", "system", "npc"]);
    expect(lines.filter((l) => !l.gmOnly).map((l) => l.text)).toEqual(["我推开门", "🎲 察觉 检定：1d20 = 14", "门后一片漆黑。", "小心。"]);
    expect(lines.filter((l) => l.warn).length).toBe(2);
  });

  it("代说转交只在主持人视角显示；台词全被删掉时不留空白叙述", () => {
    const lines = toTranscript([
      ev(1, "speech_redirected", { npcs: ["村长老格"] }),
      ev(2, "narration", { text: "", leaks: [] }),
      ev(3, "npc", { name: "村长老格", text: "唉……", leaks: [] }),
    ]);
    expect(lines.map((l) => [l.who, l.gmOnly])).toEqual([["system", true], ["npc", false]]);
    expect(lines[0].text).toContain("村长老格");
  });
});
