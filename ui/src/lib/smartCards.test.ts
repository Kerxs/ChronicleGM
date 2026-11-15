import { describe, expect, it } from "vitest";
import { cardsFrom } from "../components/SmartCards";
import type { TurnResult } from "../bindings/TurnResult";

const base = {
  quest_updates: [],
  rpg_updates: [],
  combat: null,
  checks: [],
  travel: null,
  places_heard: [],
  discoveries: [],
} as unknown as TurnResult;

describe("智能面板", () => {
  it("没有变化时不弹卡片", () => {
    expect(cardsFrom(base)).toEqual([]);
  });
  it("按回合结果弹出对应的卡片", () => {
    const r = {
      ...base,
      quest_updates: ["新任务「点亮老灯塔」"],
      rpg_updates: ["获得「备用灯芯石」", "天色暗了"],
      checks: [{ skill: "察觉", total: 14, dc: 12, degree: "success", success: true }],
      places_heard: ["潮洞"],
      discoveries: ["石台上的蓝光"],
    } as unknown as TurnResult;
    const kinds = cardsFrom(r).map((c) => c.kind);
    expect(kinds).toEqual(["quest", "character", "dice", "map", "discovery"]);
    const ch = cardsFrom(r).find((c) => c.kind === "character")!;
    expect(ch.lines).toEqual(["获得「备用灯芯石」"]);
    expect(cardsFrom(r).find((c) => c.kind === "dice")!.lines[0]).toContain("成功");
  });
});
