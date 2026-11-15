// 界面显示用的中文名称与纯函数（不含业务规则）。
import type { Op } from "../bindings/Op";
import type { ScriptItem } from "../bindings/ScriptItem";
import type { ScriptItemKind } from "../bindings/ScriptItemKind";
import type { JobStatus } from "../bindings/JobStatus";

export const ITEM_KIND: Record<ScriptItemKind, string> = {
  opening: "开场",
  chapter: "章节",
  scene: "场景",
  quest: "任务",
  clue: "线索",
  item: "道具",
  location_ref: "地点",
  key_event: "关键事件",
  trigger: "触发",
  twist: "转折",
  ending: "结局",
  handout: "玩家资料",
  player_character: "可选角色",
};

export const QUEST_TYPE: Record<string, string> = { main: "主线", side: "支线", hidden: "隐藏" };

export const JOB_KIND: Record<string, string> = {
  outline: "生成剧本大纲",
  expand_chapter: "展开章节",
  npcs: "完善人物",
  regenerate: "重写选中条目",
  extend: "继续扩展",
};

export const JOB_STATUS: Record<JobStatus, string> = {
  queued: "排队中",
  running: "生成中",
  succeeded: "已完成",
  failed: "失败",
  cancelled: "已取消",
};

export const ROUTE_TASKS: [string, string][] = [
  ["default", "默认（未单独设置的任务）"],
  ["script_gen", "剧本 / 世界 / 剧情生成"],
  ["narration", "GM 主持（叙述与判定）"],
  ["npc", "NPC 对话"],
  ["summary", "会话摘要"],
  ["analysis", "剧情导演（后台分析）"],
  ["tts", "语音朗读（本机 Breeze 或云端语音）"],
  ["image", "生成图片"],
];

export interface OpSummary {
  action: "新建" | "修改" | "删除";
  what: string;
  title: string;
  player?: string;
  gm?: string;
}

/** 把一条变更操作转换为可读摘要。 */
export function describeOp(op: Op): OpSummary {
  switch (op.op) {
    case "create_node":
      return { action: "新建", what: `世界节点·${op.node.kind}`, title: op.node.title, player: op.node.player_text, gm: op.node.gm_text };
    case "update_node":
      return { action: "修改", what: "世界节点", title: op.patch.title ?? op.id, player: op.patch.player_text ?? undefined, gm: op.patch.gm_text ?? undefined };
    case "delete_node":
      return { action: "删除", what: "世界节点", title: op.id };
    case "create_relation":
      return { action: "新建", what: "关系", title: op.relation.rel_type };
    case "delete_relation":
      return { action: "删除", what: "关系", title: op.id };
    case "create_script":
      return { action: "新建", what: "剧本", title: op.script.title, player: op.script.premise, gm: op.script.core_setting };
    case "update_script":
      return { action: "修改", what: "剧本", title: op.patch.title ?? op.id };
    case "create_script_item":
      return {
        action: "新建",
        what: ITEM_KIND[op.item.kind] ?? op.item.kind,
        title: op.item.title,
        player: op.item.player_text,
        gm: op.item.gm_text,
      };
    case "update_script_item":
      return { action: "修改", what: "条目", title: op.patch.title ?? op.id, player: op.patch.player_text ?? undefined, gm: op.patch.gm_text ?? undefined };
    case "delete_script_item":
      return { action: "删除", what: "条目", title: op.id };
    case "add_cast":
      return { action: "新建", what: "剧本角色", title: op.cast.role || "角色", gm: op.cast.secret };
    case "create_plot_node":
      return { action: "新建", what: "剧情节点", title: op.node.title };
    case "update_plot_node":
      return { action: "修改", what: "剧情节点", title: op.patch.title ?? op.id };
    case "delete_plot_node":
      return { action: "删除", what: "剧情节点", title: op.id };
    case "create_plot_edge":
      return { action: "新建", what: "剧情分支", title: op.edge.label || "分支" };
    case "delete_plot_edge":
      return { action: "删除", what: "剧情分支", title: op.id };
  }
}

export interface TreeRow {
  item: ScriptItem;
  depth: number;
}

/** 把按"父节点 → 子节点"顺序排好的扁平列表转换为带缩进层级的行。 */
export function toRows(items: ScriptItem[]): TreeRow[] {
  const depth = new Map<string, number>();
  return items.map((item) => {
    const d = item.parent_id ? (depth.get(item.parent_id) ?? 0) + 1 : 0;
    depth.set(item.id, d);
    return { item, depth: d };
  });
}

/** 某条目及其全部后代的 ID。 */
export function descendants(items: ScriptItem[], id: string): Set<string> {
  const out = new Set<string>([id]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const it of items) {
      if (it.parent_id && out.has(it.parent_id) && !out.has(it.id)) {
        out.add(it.id);
        grew = true;
      }
    }
  }
  return out;
}

export const NODE_KIND: Record<string, string> = {
  world: "世界",
  cosmos: "宇宙与背景",
  era: "时代",
  timeline_event: "时间线事件",
  region: "地区",
  nation: "国家",
  city: "城市",
  site: "特殊地点",
  faction: "势力",
  race: "种族",
  character: "人物",
  culture: "文化",
  tech_system: "科技体系",
  magic_system: "魔法体系",
  event: "事件",
  legend: "传说",
  item: "物品",
  category: "分类",
  custom: "其他",
};

export const CANON: Record<string, string> = { core: "核心设定", established: "已确立", draft: "草稿" };

export const REWRITE_MODE: Record<string, string> = { rewrite: "重写", expand: "扩写", complete: "补全空白" };
