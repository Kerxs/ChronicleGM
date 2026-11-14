// 叙述里提到的、配了图的设定。纯函数。
import type { IllustratedNode } from "../bindings/IllustratedNode";

/** 叙述里提到了哪些配了图的设定（名称或别名至少 2 个字，长名字优先，最多 4 个）。纯函数。 */
export function mentioned(text: string, nodes: IllustratedNode[], max = 4): IllustratedNode[] {
  const hits: { n: IllustratedNode; at: number; len: number }[] = [];
  for (const n of nodes) {
    let best: { at: number; len: number } | null = null;
    for (const name of [n.title, ...n.aliases]) {
      const k = name.trim();
      if (k.length < 2) continue;
      const at = text.indexOf(k);
      if (at >= 0 && (!best || k.length > best.len)) best = { at, len: k.length };
    }
    if (best) hits.push({ n, ...best });
  }
  // 名字被更长的名字包含时（"雾港" 与 "雾港税关"），只保留长的那个。
  const kept = hits.filter((h) => !hits.some((o) => o !== h && o.len > h.len && o.at <= h.at && o.at + o.len >= h.at + h.len));
  return kept
    .sort((a, b) => a.at - b.at)
    .slice(0, max)
    .map((h) => h.n);
}
