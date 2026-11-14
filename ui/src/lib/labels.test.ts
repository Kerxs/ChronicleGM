import { describe, expect, it } from "vitest";
import { describeOp, descendants, toRows } from "./labels";
import type { ScriptItem } from "../bindings/ScriptItem";

const item = (id: string, parent: string | null, title = id): ScriptItem => ({
  id, script_id: "S", kind: "scene", parent_id: parent, sort_key: "V", title, player_text: "", gm_text: "",
  payload: {}, locked: false, status: "draft", source: "user", rev: 1, created_at: 0, updated_at: 0, deleted_at: null,
});

describe("toRows / descendants", () => {
  it("按父子顺序计算缩进层级", () => {
    const items = [item("a", null), item("b", "a"), item("c", "b"), item("d", null)];
    expect(toRows(items).map((r) => r.depth)).toEqual([0, 1, 2, 0]);
    expect([...descendants(items, "a")].sort()).toEqual(["a", "b", "c"]);
  });
});

describe("describeOp", () => {
  it("新建条目显示类型与文本", () => {
    const d = describeOp({
      op: "create_script_item",
      temp_id: null,
      item: { script_id: "S", kind: "clue", parent_id: null, position: "last", title: "湿脚印", player_text: "地上有水", gm_text: "通向灯塔", payload: {}, locked: false },
    });
    expect(d).toMatchObject({ action: "新建", what: "线索", title: "湿脚印", gm: "通向灯塔" });
  });
  it("修改世界节点", () => {
    const d = describeOp({ op: "update_node", id: "N1", base_rev: 1, patch: { summary: "x" } });
    expect(d.action).toBe("修改");
  });
});
