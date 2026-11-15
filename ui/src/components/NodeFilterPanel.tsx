// 多条件筛选：按类型、名称文字、payload 字段（如稀有度、生死、年代）、关系对象、所在位置一起筛选节点。
import { useEffect, useState } from "react";
import { api } from "../api";
import type { Ctx } from "../App";
import type { FieldCond } from "../bindings/FieldCond";
import type { FieldInfo } from "../bindings/FieldInfo";
import type { Node } from "../bindings/Node";
import type { NodeKind } from "../bindings/NodeKind";
import { NODE_KIND as KIND_LABEL } from "../lib/labels";

const OPS: [string, string][] = [
  ["eq", "等于"],
  ["contains", "包含"],
  ["gte", "不小于"],
  ["lte", "不大于"],
  ["exists", "有这一项"],
];
const PRESET_KINDS: NodeKind[] = ["character", "item", "city", "site", "region", "faction", "event"];

export default function NodeFilterPanel({ ctx, onPick, onClose }: { ctx: Ctx; onPick: (n: Node) => void; onClose: () => void }) {
  const [kind, setKind] = useState<NodeKind | "">("character");
  const [text, setText] = useState("");
  const [conds, setConds] = useState<FieldCond[]>([]);
  const [fields, setFields] = useState<FieldInfo[]>([]);
  const [res, setRes] = useState<Node[] | null>(null);
  useEffect(() => {
    api.payloadFields(ctx.worldId, kind || null).then(setFields).catch(() => setFields([]));
  }, [ctx.worldId, kind]);
  const run = () =>
    void api
      .filterNodes(ctx.worldId, { kinds: kind ? [kind] : [], text, conds: conds.filter((c) => c.field && (c.op === "exists" || c.value.trim())), limit: 200 })
      .then(setRes)
      .catch((e: Error) => ctx.notify(e.message, "error"));
  const upd = (i: number, c: Partial<FieldCond>) => setConds(conds.map((x, j) => (j === i ? { ...x, ...c } : x)));
  return (
    <div className="filter-panel">
      <div className="row wrap">
        <select value={kind} onChange={(e) => setKind(e.target.value as NodeKind | "")}>
          <option value="">全部类型</option>
          {PRESET_KINDS.map((k) => (
            <option key={k} value={k}>
              {KIND_LABEL[k] ?? k}
            </option>
          ))}
        </select>
        <input value={text} onChange={(e) => setText(e.target.value)} placeholder="名称或简介包含…" style={{ flex: 1, minWidth: 100 }} />
      </div>
      {conds.map((c, i) => {
        const info = fields.find((f) => f.field === c.field);
        return (
          <div key={i} className="row wrap cond">
            <select value={c.field} onChange={(e) => upd(i, { field: e.target.value, value: "" })}>
              <option value="">选择条件…</option>
              <option value="under">在…下面（节点 ID）</option>
              <option value="related">和…有关系（节点 ID）</option>
              {fields.map((f) => (
                <option key={f.field} value={f.field}>
                  {f.field}（{f.count}）
                </option>
              ))}
            </select>
            {c.field !== "under" && c.field !== "related" && (
              <select value={c.op} onChange={(e) => upd(i, { op: e.target.value })}>
                {OPS.filter(([o]) => info?.numeric || (o !== "gte" && o !== "lte")).map(([o, l]) => (
                  <option key={o} value={o}>
                    {l}
                  </option>
                ))}
              </select>
            )}
            {c.op !== "exists" && (
              <input list={`fv-${i}`} value={c.value} onChange={(e) => upd(i, { value: e.target.value })} placeholder="值" style={{ width: 110 }} />
            )}
            <datalist id={`fv-${i}`}>
              {(info?.samples ?? []).map((v) => (
                <option key={v} value={v} />
              ))}
            </datalist>
            <button className="link" onClick={() => setConds(conds.filter((_, j) => j !== i))}>
              ✕
            </button>
          </div>
        );
      })}
      <div className="row">
        <button className="small" onClick={() => setConds([...conds, { field: "", op: "eq", value: "" }])}>
          ＋ 条件
        </button>
        <button className="small primary" onClick={run}>
          筛选
        </button>
        <span className="spacer" />
        <button className="link" onClick={onClose}>
          收起
        </button>
      </div>
      {res && (
        <div className="filter-results">
          <div className="hint">{res.length ? `${res.length} 个结果` : "没有符合条件的节点"}</div>
          {res.map((n) => (
            <button key={n.id} className="link filter-hit" onClick={() => onPick(n)}>
              <span className="kind">{KIND_LABEL[n.kind] ?? n.kind}</span> {n.title}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
