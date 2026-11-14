import { describe, expect, it } from "vitest";
import { ROOT, visibleRows, windowRange } from "./tree";
import type { TreeNode } from "../api";

const tn = (id: string, children = 0): TreeNode => ({
  node: { id, kind: "city", parent_id: null, sort_key: "V", path: `/${id}/`, title: id, aliases: [], summary: "", player_text: "", gm_text: "",
    payload: {}, tags: [], canon_level: "draft", locked: false, status: "draft", source: "user", rev: 1, created_at: 0, updated_at: 0, deleted_at: null },
  child_count: children,
});

describe("visibleRows", () => {
  it("只展开已展开且有子节点的行，并显示加载更多与加载中", () => {
    const cache = new Map<string, TreeNode[]>([
      [ROOT, [tn("a", 3), tn("b", 2), tn("c")]],
      ["a", [tn("a1"), tn("a2")]],
    ]);
    const totals = new Map([[ROOT, 3], ["a", 3]]);
    const rows = visibleRows(cache, totals, new Set(["a", "b", "c"]));
    expect(rows.map((r) => (r.type === "node" ? r.item.node.id : r.type))).toEqual(["a", "a1", "a2", "more", "b", "loading", "c"]);
    expect(rows.find((r) => r.type === "node" && r.item.node.id === "c")).toMatchObject({ expanded: false });
    expect(visibleRows(cache, totals, new Set()).length).toBe(3);
  });
});

describe("windowRange", () => {
  it("只渲染视口附近的行", () => {
    expect(windowRange(50000, 26000, 520, 26, 5)).toEqual([995, 1025]);
    expect(windowRange(10, 0, 520, 26)).toEqual([0, 10]);
  });
});
