import { describe, expect, it } from "vitest";
import { placeGuide } from "./Tutorial";

const size = { w: 406, h: 230 };
const hit = (p: { x: number; y: number }, t: { x: number; y: number; w: number; h: number }) =>
  p.x < t.x + t.w && t.x < p.x + size.w && p.y < t.y + t.h && t.y < p.y + size.h;

describe("教程引导的位置", () => {
  it("不挡住目标：右上角工具栏（选剧本 + 新建战役）", () => {
    const t = { x: 700, y: 60, w: 520, h: 34 };
    const p = placeGuide(t, size, 1280, 800);
    expect(hit(p, t)).toBe(false);
    expect(p.x).toBeGreaterThanOrEqual(12);
    expect(p.x + size.w).toBeLessThanOrEqual(1268);
  });
  it("目标在左边时放在右边", () => {
    const t = { x: 10, y: 200, w: 180, h: 40 };
    expect(placeGuide(t, size, 1280, 800).side).toBe("right");
  });
  it("窗口里所有位置都会挡住时也不出窗口", () => {
    const t = { x: 0, y: 0, w: 1280, h: 700 };
    const p = placeGuide(t, size, 1280, 800);
    expect(p.y + size.h).toBeLessThanOrEqual(800);
    expect(p.y).toBeGreaterThanOrEqual(0);
  });
  it("没有目标时在中间", () => {
    const p = placeGuide(null, size, 1280, 800);
    expect(Math.abs(p.x + size.w / 2 - 640)).toBeLessThan(2);
  });
});
