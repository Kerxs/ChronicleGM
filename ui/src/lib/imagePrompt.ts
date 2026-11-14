// 生图描述：按节点设定拼出来。纯函数。
import type { Node } from "../bindings/Node";
import { NODE_KIND } from "./labels";

/** 按节点设定拼出图片描述。只用标题、类型、摘要与玩家可见描述（主持人隐藏内容不会发给生图服务）。纯函数。 */
export function promptFor(node: Pick<Node, "title" | "kind" | "summary" | "player_text">, style: string): string {
  const kind = NODE_KIND[node.kind] ?? node.kind;
  const what = node.kind === "character" ? "人物肖像" : ["city", "site", "region", "nation"].includes(node.kind) ? "场景" : node.kind === "item" ? "物品特写" : "概念图";
  const desc = [node.summary, node.player_text].map((t) => t.trim()).filter(Boolean).join("。").slice(0, 600);
  return [`${kind}「${node.title}」的${what}。`, desc, style ? `${style}。` : "", "画面中不要出现文字。"].filter(Boolean).join("\n");
}
