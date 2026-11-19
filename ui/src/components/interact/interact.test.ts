import { describe, expect, it } from "vitest";
import { pinScore } from "./LockPick";
import { pickCues } from "../ritual/RitualLayer";
import { ambientFor } from "../../lib/sfx";
import type { RitualCue } from "../../bindings/RitualCue";

describe("撬锁小游戏", () => {
  it("越靠近中心得分越高，离开区域很快降到 0", () => {
    expect(pinScore(0.5, 0.5, 0.2)).toBe(1);
    expect(pinScore(0.6, 0.5, 0.2)).toBeCloseTo(0.5);
    expect(pinScore(0.95, 0.5, 0.2)).toBe(0);
    expect(pinScore(0.55, 0.5, 0.2)).toBeGreaterThan(pinScore(0.58, 0.5, 0.2));
  });
});

describe("仪式演出", () => {
  const c = (level: number, scene: string): RitualCue => ({ level, scene, title: scene, subtitle: "", node_id: null, sound: scene });
  it("一级、二级不演出；四级优先；最多三个", () => {
    const got = pickCues([c(1, "dice"), c(3, "npc"), c(4, "prologue"), c(3, "clue"), c(3, "item")], "full");
    expect(got.map((x) => x.scene)).toEqual(["prologue", "npc", "clue"]);
  });
  it("简化时四级也只用卡片；关闭时不演出", () => {
    expect(pickCues([c(4, "ending")], "simple")[0].level).toBe(3);
    expect(pickCues([c(4, "ending")], "off")).toEqual([]);
  });
});

describe("环境音", () => {
  it("按地点和天气猜", () => {
    expect(ambientFor("雾灯镇码头", "")).toBe("sea");
    expect(ambientFor("潮洞", "")).toBe("cave");
    expect(ambientFor("黑木镇", "")).toBe("town");
    expect(ambientFor("荒野", "暴雨")).toBe("rain");
  });
});

import { splitD100 } from "../../lib/d100";
describe("百分骰拆成两颗 d10", () => {
  it("十位和个位", () => {
    expect(splitD100(57)).toEqual({ tens: 5, ones: 7 });
    expect(splitD100(100)).toEqual({ tens: 0, ones: 0 });
    expect(splitD100(5)).toEqual({ tens: 0, ones: 5 });
  });
});
