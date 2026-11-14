import { describe, expect, it } from "vitest";
import { COND_LABEL, EFF_LABEL, defaultCondition, defaultEffect, describeCondition, describeEffect, layeredLayout } from "./dsl";
import type { Condition } from "../bindings/Condition";

const names = (id: string) => ({ Q1: "调查遗迹", C1: "石板符文", N1: "村长", P1: "遗迹" })[id] ?? id;

describe("describeCondition", () => {
  it("把嵌套条件转成可读文字", () => {
    const c: Condition = {
      type: "all",
      of: [
        { type: "quest_state", quest: "Q1", state: "active" },
        { type: "any", of: [{ type: "clue_known", clue: "C1" }, { type: "check", actor: "party", skill: "察觉", dc: 15, dice: "1d20" }] },
        { type: "not", cond: { type: "flag", name: "refused" } },
      ],
    };
    expect(describeCondition(c, names)).toBe("任务「调查遗迹」进行中 且 （已知线索「石板符文」 或 察觉检定 DC 15） 且 并非（标志「refused」）");
    expect(describeCondition(null, names)).toBe("总是可走");
  });
});

describe("describeEffect", () => {
  it("效果描述", () => {
    expect(describeEffect({ type: "inc_var", key: "gold", by: -5 }, names)).toBe("gold -5");
    expect(describeEffect({ type: "set_relation", a: "party", b: "N1", rel_type: "trusts", active: true }, names)).toBe("队伍 与 村长：建立 trusts");
  });
});

describe("defaults", () => {
  it("每种条件与效果都有合法默认值", () => {
    for (const t of Object.keys(COND_LABEL) as (keyof typeof COND_LABEL)[]) expect(defaultCondition(t).type).toBe(t);
    for (const t of Object.keys(EFF_LABEL) as (keyof typeof EFF_LABEL)[]) expect(defaultEffect(t).type).toBe(t);
  });
});

describe("layeredLayout", () => {
  it("按距离开场分层，孤立节点放最后一层", () => {
    const pos = layeredLayout(["s", "a", "b", "c", "x"], "s", [["s", "a"], ["s", "b"], ["a", "c"]]);
    expect(pos.get("s")).toEqual({ x: 0, y: 0 });
    expect(pos.get("a")!.x).toBe(260);
    expect(pos.get("b")).toEqual({ x: 260, y: 120 });
    expect(pos.get("c")!.x).toBe(520);
    expect(pos.get("x")!.x).toBe(780);
  });
});
