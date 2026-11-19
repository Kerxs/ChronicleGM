// 剧情条件与效果的界面辅助（纯函数）：默认值、可读描述。规则本身在 Rust 中求值。
import type { Condition } from "../bindings/Condition";
import type { Effect } from "../bindings/Effect";
import type { Cmp } from "../bindings/Cmp";

export type CondType = Condition["type"];
export type EffType = Effect["type"];

export const COND_LABEL: Record<CondType, string> = {
  const: "固定值",
  all: "全部满足",
  any: "任一满足",
  not: "不满足",
  var: "变量比较",
  flag: "标志已设置",
  quest_state: "任务状态",
  has_item: "持有物品",
  clue_known: "已知线索",
  alive: "人物还活着",
  relation: "存在关系",
  visited: "到过节点",
  check: "技能检定（程序掷骰）",
};

export const EFF_LABEL: Record<EffType, string> = {
  set_var: "设置变量",
  inc_var: "增减变量",
  set_flag: "设置标志",
  set_quest_state: "设置任务状态",
  give_item: "获得物品",
  take_item: "失去物品",
  reveal_clue: "获得线索",
  set_relation: "设置关系",
  set_entity_state: "改变实体状态",
  unlock_node: "解锁剧情节点",
  spawn: "引入已批准的内容",
  adjust_bond: "调整人物关系",
  adjust_mood: "调整人物情绪",
  upsert_record: "写入战役记录（程序使用）",
  delete_record: "删除战役记录（程序使用）",
};

/** 只由程序使用、不在编辑器里提供的效果。 */
export const PROGRAM_ONLY_EFFECTS: EffType[] = ["upsert_record", "delete_record"];

export const CMP_LABEL: Record<Cmp, string> = { eq: "=", ne: "≠", lt: "<", le: "≤", gt: ">", ge: "≥", contains: "包含", exists: "存在" };

export function defaultCondition(t: CondType): Condition {
  switch (t) {
    case "const":
      return { type: "const", value: true };
    case "all":
      return { type: "all", of: [] };
    case "any":
      return { type: "any", of: [] };
    case "not":
      return { type: "not", cond: { type: "flag", name: "" } };
    case "var":
      return { type: "var", key: "", op: "ge", value: 1 };
    case "flag":
      return { type: "flag", name: "" };
    case "quest_state":
      return { type: "quest_state", quest: "", state: "active" };
    case "has_item":
      return { type: "has_item", holder: "party", item: "", min_qty: 1 };
    case "clue_known":
      return { type: "clue_known", clue: "" };
    case "alive":
      return { type: "alive", entity: "" };
    case "relation":
      return { type: "relation", a: "party", b: "", rel_type: "trusts" };
    case "visited":
      return { type: "visited", node: "", min_count: 1 };
    case "check":
      return { type: "check", actor: "party", skill: "", dc: 12, dice: "1d20" };
  }
}

export function defaultEffect(t: EffType): Effect {
  switch (t) {
    case "set_var":
      return { type: "set_var", key: "", value: 0 };
    case "inc_var":
      return { type: "inc_var", key: "", by: 1 };
    case "set_flag":
      return { type: "set_flag", name: "", value: true };
    case "set_quest_state":
      return { type: "set_quest_state", quest: "", state: "active" };
    case "give_item":
      return { type: "give_item", holder: "party", item: "", qty: 1 };
    case "take_item":
      return { type: "take_item", holder: "party", item: "", qty: 1 };
    case "reveal_clue":
      return { type: "reveal_clue", clue: "" };
    case "set_relation":
      return { type: "set_relation", a: "party", b: "", rel_type: "trusts", active: true };
    case "set_entity_state":
      return { type: "set_entity_state", entity: "", field: "life_status", value: "dead" };
    case "unlock_node":
      return { type: "unlock_node", node: "" };
    case "spawn":
      return { type: "spawn", changeset: "" };
    case "adjust_bond":
      return { type: "adjust_bond", from: "", to: "party", kind: "", strength: 0, trust: 0, affection: 0, hostility: 0, reason: "", mutual: false };
    case "adjust_mood":
      return { type: "adjust_mood", npc: "", fear: 0, anger: 0, suspicion: 0, stress: 0, loyalty: 0, reason: "" };
    case "upsert_record":
      return { type: "upsert_record", kind: "", id: "", data: {} };
    case "delete_record":
      return { type: "delete_record", kind: "", id: "" };
  }
}

/** 把 ID 显示成名称；找不到时原样显示。 */
export type Names = (id: string) => string;

const QUEST_STATE: Record<string, string> = { active: "进行中", done: "已完成", failed: "已失败", refused: "已拒绝", bypassed: "已绕过", invalid: "已失效", changed: "被玩家改变" };

export function describeCondition(c: Condition | null, name: Names): string {
  if (!c) return "总是可走";
  switch (c.type) {
    case "const":
      return c.value ? "总是" : "从不";
    case "all":
      return c.of.length === 0 ? "（空）" : c.of.map((x) => wrap(x, name)).join(" 且 ");
    case "any":
      return c.of.length === 0 ? "（空）" : c.of.map((x) => wrap(x, name)).join(" 或 ");
    case "not":
      return `并非（${describeCondition(c.cond, name)}）`;
    case "var":
      return c.op === "exists" ? `变量 ${c.key} 存在` : `${c.key} ${CMP_LABEL[c.op]} ${JSON.stringify(c.value)}`;
    case "flag":
      return `标志「${c.name}」`;
    case "quest_state":
      return `任务「${name(c.quest)}」${QUEST_STATE[c.state] ?? c.state}`;
    case "has_item":
      return `${c.holder === "party" ? "队伍" : name(c.holder)}持有「${name(c.item)}」×${c.min_qty}`;
    case "clue_known":
      return `已知线索「${name(c.clue)}」`;
    case "alive":
      return `「${name(c.entity)}」还活着`;
    case "relation":
      return `${c.a === "party" ? "队伍" : name(c.a)} 与 ${name(c.b)}：${c.rel_type}`;
    case "visited":
      return `到过「${name(c.node)}」${c.min_count > 1 ? ` ${c.min_count} 次` : ""}`;
    case "check":
      return `${c.skill || "技能"}检定 DC ${c.dc}`;
  }
}

function wrap(c: Condition, name: Names): string {
  const s = describeCondition(c, name);
  return c.type === "all" || c.type === "any" ? `（${s}）` : s;
}

export function describeEffect(e: Effect, name: Names): string {
  switch (e.type) {
    case "set_var":
      return `${e.key} = ${JSON.stringify(e.value)}`;
    case "inc_var":
      return `${e.key} ${e.by >= 0 ? "+" : ""}${e.by}`;
    case "set_flag":
      return `${e.value ? "设置" : "清除"}标志「${e.name}」`;
    case "set_quest_state":
      return `任务「${name(e.quest)}」→ ${QUEST_STATE[e.state] ?? e.state}`;
    case "give_item":
      return `获得「${name(e.item)}」×${e.qty}`;
    case "take_item":
      return `失去「${name(e.item)}」×${e.qty}`;
    case "reveal_clue":
      return `获得线索「${name(e.clue)}」`;
    case "set_relation":
      return `${e.a === "party" ? "队伍" : name(e.a)} 与 ${name(e.b)}：${e.active ? "建立" : "解除"} ${e.rel_type}`;
    case "set_entity_state":
      return `${name(e.entity)}.${e.field} = ${JSON.stringify(e.value)}`;
    case "unlock_node":
      return `解锁「${name(e.node)}」`;
    case "spawn":
      return `引入变更集 ${e.changeset.slice(-6)}`;
    case "adjust_bond": {
      const who = (x: string) => (x === "party" ? "玩家" : name(x));
      const parts = (
        [
          ["强度", e.strength],
          ["信任", e.trust],
          ["好感", e.affection],
          ["敌意", e.hostility],
        ] as [string, number][]
      )
        .filter(([, v]) => v !== 0)
        .map(([k, v]) => `${k}${v > 0 ? "+" : ""}${v}`);
      return `${who(e.from)} 对 ${who(e.to)}${e.mutual ? "（双方）" : ""}：${parts.join("，") || "不变"}${e.kind ? `，关系变为 ${e.kind}` : ""}（${e.reason || "未写原因"}）`;
    }
    case "adjust_mood": {
      const parts = (
        [
          ["恐惧", e.fear],
          ["愤怒", e.anger],
          ["怀疑", e.suspicion],
          ["压力", e.stress],
          ["忠诚", e.loyalty],
        ] as [string, number][]
      )
        .filter(([, v]) => v !== 0)
        .map(([k, v]) => `${k}${v > 0 ? "+" : ""}${v}`);
      return `${name(e.npc)} 的情绪：${parts.join("，") || "不变"}（${e.reason || "未写原因"}）`;
    }
    case "upsert_record":
      return `记录 ${e.kind}`;
    case "delete_record":
      return `删除记录 ${e.kind}`;
  }
}

/** 按到开场节点的距离分层排布（用于还没有手动摆放位置的节点）。 */
export function layeredLayout(nodeIds: string[], start: string | null, edges: [string, string][]): Map<string, { x: number; y: number }> {
  const depth = new Map<string, number>();
  if (start) {
    depth.set(start, 0);
    const q = [start];
    while (q.length) {
      const cur = q.shift()!;
      for (const [a, b] of edges) {
        if (a === cur && !depth.has(b)) {
          depth.set(b, depth.get(cur)! + 1);
          q.push(b);
        }
      }
    }
  }
  const maxD = Math.max(0, ...depth.values());
  const rows = new Map<number, number>();
  const pos = new Map<string, { x: number; y: number }>();
  for (const id of nodeIds) {
    const d = depth.get(id) ?? maxD + 1;
    const r = rows.get(d) ?? 0;
    rows.set(d, r + 1);
    pos.set(id, { x: d * 260, y: r * 120 });
  }
  return pos;
}
