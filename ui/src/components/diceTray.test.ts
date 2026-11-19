import { describe, expect, it } from "vitest";
import { formula, takeBatch } from "./DiceTray";
import { labelFaces, settleForTest } from "../lib/dice3d";
import type { DiceRecord } from "../bindings/DiceRecord";

const rec = (over: Partial<DiceRecord>): DiceRecord => ({ purpose: "x", category: "check", expr: "1d20", dice: [14], sides: [20], kept: [0], natural: 14, parts: [], total: 14, target: null, outcome: "", stream: 1, manual: false, ...over });

describe("3D 骰子的面", () => {
  it("落地朝上的面印真实点数，其他面不重复印它", () => {
    for (const sides of [6, 8, 10, 12, 20]) {
      for (const value of [1, sides]) {
        const up = sides - 1;
        const labels = labelFaces(sides, value, up, 0);
        const shown = sides === 10 && value === 10 ? "0" : String(value);
        expect(labels[up]).toBe(shown);
        expect(labels.filter((l) => l === shown)).toHaveLength(1);
        expect(labels).toHaveLength(sides);
      }
    }
  });
  it("真实物理模拟后，朝上那面就是规则引擎给的点数（每种骰子投 10 次）", () => {
    for (const sides of [4, 6, 8, 10, 12, 20]) {
      for (let k = 0; k < 10; k++) {
        const value = 1 + ((k * 7) % sides);
        const [{ shown, flat }] = settleForTest([{ sides, value, color: "#333" }]);
        const want = sides === 10 && value === 10 ? "0" : String(value);
        expect(shown.every((l) => l === want), `d${sides}=${value}: ${shown}`).toBe(true);
        expect(flat, `d${sides} 停下时应平放在桌上（不是立在棱上）`).toBeGreaterThan(0.97);
      }
    }
  });
  it("d4 读压在下面那面之外的三个面", () => {
    const labels = labelFaces(4, 3, 0, 2);
    expect(labels.filter((l) => l === "3")).toHaveLength(3);
    expect(labels[2]).not.toBe("3");
  });
});

describe("骰子托盘", () => {
  it("算式写明骰面、加值、目标与结果", () => {
    const r = rec({ category: "attack", parts: [["攻击加值", 3], ["状态", -1]], total: 16, target: 15, outcome: "命中" });
    expect(formula(r)).toBe("1d20 14 + 攻击加值 3 − 状态 1 = 16 ／ 防御 15 → 命中");
    expect(formula(rec({ expr: "2d20kh1", dice: [5, 17], kept: [1], natural: 17 }))).toContain("(5) 17");
  });
  it("一批最多 8 颗骰子，单条记录超过也照样放进一批", () => {
    const q = [rec({}), rec({ dice: [1, 2, 3, 4, 5, 6], sides: [6, 6, 6, 6, 6, 6] }), rec({}), rec({})];
    expect(takeBatch(q)).toBe(3);
    expect(takeBatch([rec({ dice: Array(12).fill(3), sides: Array(12).fill(6) })])).toBe(1);
  });
});
