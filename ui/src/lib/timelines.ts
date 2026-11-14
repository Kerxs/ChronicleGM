// 把战役按时间线谱系排成树（父时间线在前，子时间线缩进跟在后面）。纯函数。
import type { Campaign } from "../bindings/Campaign";

export interface TimelineRow {
  campaign: Campaign;
  depth: number;
  /** 根时间线的 ID（同一家族的时间线可以互相比较）。 */
  root: string;
}

export function timelineTree(list: Campaign[], showArchived: boolean): TimelineRow[] {
  const byParent = new Map<string | null, Campaign[]>();
  const ids = new Set(list.map((c) => c.id));
  for (const c of list) {
    // 父时间线不在列表里时当作根。
    const p = c.parent_id && ids.has(c.parent_id) ? c.parent_id : null;
    byParent.set(p, [...(byParent.get(p) ?? []), c]);
  }
  const out: TimelineRow[] = [];
  const walk = (parent: string | null, depth: number, root: string | null) => {
    const kids = (byParent.get(parent) ?? []).sort((a, b) => a.created_at - b.created_at);
    for (const c of kids) {
      const r = root ?? c.id;
      if (showArchived || !c.archived) out.push({ campaign: c, depth, root: r });
      walk(c.id, depth + 1, r);
    }
  };
  walk(null, 0, null);
  return out;
}

export function timelineLabel(r: TimelineRow): string {
  const c = r.campaign;
  const pad = "　".repeat(r.depth) + (r.depth > 0 ? "└ " : "");
  const status = c.status === "ended" ? `已结束：${c.ending || "—"}` : `回合 ${c.turn}`;
  return `${pad}${c.name}（${status}${c.archived ? "，已归档" : ""}）`;
}
