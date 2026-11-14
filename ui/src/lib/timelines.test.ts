import { describe, expect, it } from "vitest";
import { timelineTree, timelineLabel } from "./timelines";
import type { Campaign } from "../bindings/Campaign";

const c = (id: string, parent: string | null, at: number, extra: Partial<Campaign> = {}): Campaign => ({
  id, script_id: null, name: id, base_commit: null, rng_seed: 1, roll_counter: 0, turn: 0, current_plot_node: null,
  created_at: at, updated_at: at, parent_id: parent, fork_seq: null, fork_turn: null, status: "active", archived: false, ended_at: null, ending: "", ...extra,
});

describe("timelineTree", () => {
  it("父时间线在前，分支缩进；归档的默认隐藏但不打断子树", () => {
    const list = [c("b2", "a", 3), c("a", null, 1, { status: "ended", ending: "封印加固" }), c("b1", "a", 2, { archived: true }), c("b1x", "b1", 4), c("z", null, 5)];
    expect(timelineTree(list, true).map((r) => [r.campaign.id, r.depth, r.root])).toEqual([
      ["a", 0, "a"], ["b1", 1, "a"], ["b1x", 2, "a"], ["b2", 1, "a"], ["z", 0, "z"],
    ]);
    expect(timelineTree(list, false).map((r) => r.campaign.id)).toEqual(["a", "b1x", "b2", "z"]);
    expect(timelineLabel(timelineTree(list, true)[0])).toBe("a（已结束：封印加固）");
    expect(timelineLabel(timelineTree(list, true)[2])).toContain("└ b1x");
  });
});
