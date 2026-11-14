import { describe, expect, it } from "vitest";
import { radialLayout } from "./graph";

describe("radialLayout", () => {
  it("中心在原点，一度节点在内圈，二度在外圈，孤立节点也有位置", () => {
    const pos = radialLayout("c", ["c", "a", "b", "x", "lonely"], [["c", "a"], ["b", "c"], ["a", "x"]]);
    const r = (id: string) => Math.hypot(pos.get(id)!.x, pos.get(id)!.y);
    expect(pos.get("c")).toEqual({ x: 0, y: 0 });
    expect(r("a")).toBeCloseTo(r("b"), 0);
    expect(r("x")).toBeGreaterThan(r("a"));
    expect(pos.has("lonely")).toBe(true);
    const keys = new Set([...pos.values()].map((p) => `${p.x},${p.y}`));
    expect(keys.size).toBe(5);
  });
});
