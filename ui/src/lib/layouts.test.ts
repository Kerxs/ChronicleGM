import { describe, expect, it } from "vitest";
import { forceLayout } from "./forceLayout";
import { treeLayout } from "./treeLayout";

describe("forceLayout", () => {
  it("确定性、节点不重叠、相连的节点更近", () => {
    const ids = ["a", "b", "c", "d", "e"];
    const links: [string, string][] = [["a", "b"], ["b", "c"]];
    const p1 = forceLayout(ids, links);
    const p2 = forceLayout(ids, links);
    expect([...p1.entries()]).toEqual([...p2.entries()]);
    const d = (x: string, y: string) => Math.hypot(p1.get(x)!.x - p1.get(y)!.x, p1.get(x)!.y - p1.get(y)!.y);
    for (const a of ids) for (const b of ids) if (a < b) expect(d(a, b)).toBeGreaterThan(30);
    expect(d("a", "b")).toBeLessThan(d("a", "e"));
  });
  it("500 个节点在合理时间内完成", () => {
    const ids = Array.from({ length: 500 }, (_, i) => `n${i}`);
    const links = ids.slice(1).map((id, i) => [ids[i], id] as [string, string]);
    const t = performance.now();
    expect(forceLayout(ids, links).size).toBe(500);
    expect(performance.now() - t).toBeLessThan(3000);
  });
});

describe("treeLayout", () => {
  it("子节点在父节点右侧，父节点居中，兄弟不重叠", () => {
    const p = treeLayout([{ id: "r", parent: null }, { id: "a", parent: "r" }, { id: "b", parent: "r" }, { id: "a1", parent: "a" }, { id: "r2", parent: null }]);
    expect(p.get("a")!.x).toBeGreaterThan(p.get("r")!.x);
    expect(p.get("a1")!.x).toBeGreaterThan(p.get("a")!.x);
    expect(p.get("r")!.y).toBe((p.get("a")!.y + p.get("b")!.y) / 2);
    expect(p.get("a")!.y).not.toBe(p.get("b")!.y);
    expect(p.get("r2")!.y).toBeGreaterThan(p.get("b")!.y);
  });

  it("很多子节点时分多列排列，高度受限", () => {
    const items = [{ id: "cat", parent: null }, ...Array.from({ length: 300 }, (_, i) => ({ id: `c${i}`, parent: "cat" }))];
    const p = treeLayout(items, { rowHeight: 50, perColumn: 30 });
    const ys = items.slice(1).map((i) => p.get(i.id)!.y);
    const xs = new Set(items.slice(1).map((i) => p.get(i.id)!.x));
    expect(Math.max(...ys)).toBeLessThan(30 * 50);
    expect(xs.size).toBe(10);
    const keys = new Set(items.slice(1).map((i) => `${p.get(i.id)!.x},${p.get(i.id)!.y}`));
    expect(keys.size).toBe(300);
  });
});

describe("forceLayout 孤立人物", () => {
  it("几个互不相连的人物不会被推得很远", () => {
    const ids = ["a", "b", "c", "d", "e"];
    const p = forceLayout(ids, [["c", "d"]]);
    const xs = ids.map((i) => p.get(i)!.x);
    const ys = ids.map((i) => p.get(i)!.y);
    const span = Math.max(Math.max(...xs) - Math.min(...xs), Math.max(...ys) - Math.min(...ys));
    expect(span).toBeLessThan(140 * 4);
    // 也不会挤在一起
    for (const a of ids) for (const b of ids) if (a < b) expect(Math.hypot(p.get(a)!.x - p.get(b)!.x, p.get(a)!.y - p.get(b)!.y)).toBeGreaterThan(60);
  });
});
