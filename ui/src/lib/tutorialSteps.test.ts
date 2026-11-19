import { describe, expect, it, vi } from "vitest";

vi.mock("../api", () => ({ api: {} }));
import { TOUR } from "./tutorialSteps";

/** 界面源码里出现的全部 data-tour 锚点（含导航按钮 nav-${icon}）。用 Vite 的 glob 读源码文本。 */
const SOURCES = import.meta.glob("../**/*.tsx", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
function anchors(): Set<string> {
  const out = new Set<string>();
  for (const s of Object.values(SOURCES)) {
    for (const m of s.matchAll(/data-tour="([^"]+)"/g)) out.add(m[1]);
    for (const m of s.matchAll(/<NavButton icon="([^"]+)"/g)) out.add(`nav-${m[1]}`);
    // 游戏页的页签：data-tour={`tab-${k}`}，按页签列表展开。
    if (s.includes("data-tour={`tab-${k}`}")) for (const m of s.matchAll(/\["([a-z]+)", (?:"[^"]+"|immersive \? "[^"]+" : "[^"]+")(?:, "[^"]+")?\]/g)) out.add(`tab-${m[1]}`);
  }
  return out;
}

describe("游戏教程步骤", () => {
  it("编号不重复，等待的步骤都有提示", () => {
    expect(new Set(TOUR.map((s) => s.id)).size).toBe(TOUR.length);
    for (const s of TOUR) {
      if (s.until) expect(s.waiting, s.id).toBeTruthy();
      expect(s.say.length, s.id).toBeGreaterThan(10);
    }
  });
  it("指向的元素在界面里都有锚点", () => {
    const all = anchors();
    for (const s of TOUR) for (const t of s.target ?? []) expect(all.has(t), `${s.id} → ${t}`).toBe(true);
  });
  it("需要模型的步骤可以跳过，不会把没有模型的人卡住", () => {
    for (const id of ["models", "opening"]) expect(TOUR.find((s) => s.id === id)?.skipLabel, id).toBeTruthy();
  });
});
