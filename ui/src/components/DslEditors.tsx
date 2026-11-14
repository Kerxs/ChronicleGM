// 条件与效果的表单编辑器。底层数据就是规则 DSL 的 JSON；求值与执行在 Rust 中进行。
import type { Condition } from "../bindings/Condition";
import type { Effect } from "../bindings/Effect";
import type { Cmp } from "../bindings/Cmp";
import { PROGRAM_ONLY_EFFECTS, CMP_LABEL, COND_LABEL, EFF_LABEL, defaultCondition, defaultEffect, type CondType, type EffType } from "../lib/dsl";

export interface Option {
  id: string;
  label: string;
}

/** 编辑器需要的可选项：剧本任务、线索、物品，剧情节点，人物。 */
export interface Choices {
  quests: Option[];
  clues: Option[];
  items: Option[];
  plotNodes: Option[];
  people: Option[];
}

function Pick({ value, options, onChange, allowParty }: { value: string; options: Option[]; onChange: (v: string) => void; allowParty?: boolean }) {
  const known = options.some((o) => o.id === value) || (allowParty && value === "party");
  return (
    <select value={value} onChange={(e) => onChange(e.target.value)}>
      <option value="">（请选择）</option>
      {allowParty && <option value="party">队伍（玩家）</option>}
      {options.map((o) => (
        <option key={o.id} value={o.id}>
          {o.label}
        </option>
      ))}
      {!known && value && <option value={value}>{value}（不存在）</option>}
    </select>
  );
}

function Num({ value, onChange, step }: { value: number; onChange: (v: number) => void; step?: number }) {
  return <input type="number" step={step ?? 1} value={value} onChange={(e) => onChange(Number(e.target.value))} style={{ width: 70 }} />;
}

function Txt({ value, onChange, placeholder, width }: { value: string; onChange: (v: string) => void; placeholder?: string; width?: number }) {
  return <input value={value} placeholder={placeholder} onChange={(e) => onChange(e.target.value)} style={{ width: width ?? 110 }} />;
}

function StateInput({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return (
    <select value={value} onChange={(e) => onChange(e.target.value)}>
      {["active", "done", "failed"].map((s) => (
        <option key={s} value={s}>
          {{ active: "进行中", done: "已完成", failed: "已失败" }[s]}
        </option>
      ))}
      {!["active", "done", "failed"].includes(value) && <option value={value}>{value}</option>}
    </select>
  );
}

function parseValue(s: string): unknown {
  try {
    return JSON.parse(s);
  } catch {
    return s;
  }
}

export function ConditionEditor({ value, onChange, choices, depth = 0 }: { value: Condition; onChange: (c: Condition) => void; choices: Choices; depth?: number }) {
  const set = (patch: Partial<Condition>) => onChange({ ...value, ...patch } as Condition);
  const typeSelect = (
    <select value={value.type} onChange={(e) => onChange(defaultCondition(e.target.value as CondType))}>
      {Object.entries(COND_LABEL).map(([k, v]) => (
        <option key={k} value={k}>
          {v}
        </option>
      ))}
    </select>
  );
  let body: React.ReactNode = null;
  switch (value.type) {
    case "all":
    case "any": {
      const c = value;
      body = (
        <div className="cond-children">
          {c.of.map((child, i) => (
            <div key={i} className="row">
              <ConditionEditor
                value={child}
                choices={choices}
                depth={depth + 1}
                onChange={(n) => onChange({ ...c, of: c.of.map((x, j) => (j === i ? n : x)) })}
              />
              <button className="link danger" onClick={() => onChange({ ...c, of: c.of.filter((_, j) => j !== i) })}>
                ×
              </button>
            </div>
          ))}
          <button className="link" onClick={() => onChange({ ...c, of: [...c.of, defaultCondition("flag")] })}>
            + 添加条件
          </button>
        </div>
      );
      break;
    }
    case "not":
      body = <ConditionEditor value={value.cond} choices={choices} depth={depth + 1} onChange={(n) => set({ cond: n })} />;
      break;
    case "const":
      body = (
        <select value={String(value.value)} onChange={(e) => set({ value: e.target.value === "true" })}>
          <option value="true">真</option>
          <option value="false">假</option>
        </select>
      );
      break;
    case "var":
      body = (
        <>
          <Txt value={value.key} onChange={(v) => set({ key: v })} placeholder="变量名" />
          <select value={value.op} onChange={(e) => set({ op: e.target.value as Cmp })}>
            {Object.entries(CMP_LABEL).map(([k, v]) => (
              <option key={k} value={k}>
                {v}
              </option>
            ))}
          </select>
          {value.op !== "exists" && <Txt value={JSON.stringify(value.value)} onChange={(v) => set({ value: parseValue(v) as never })} width={70} />}
        </>
      );
      break;
    case "flag":
      body = <Txt value={value.name} onChange={(v) => set({ name: v })} placeholder="标志名" />;
      break;
    case "quest_state":
      body = (
        <>
          <Pick value={value.quest} options={choices.quests} onChange={(v) => set({ quest: v })} />
          <StateInput value={value.state} onChange={(v) => set({ state: v })} />
        </>
      );
      break;
    case "has_item":
      body = (
        <>
          <Pick value={value.item} options={choices.items} onChange={(v) => set({ item: v })} />× <Num value={value.min_qty} onChange={(v) => set({ min_qty: v })} />
        </>
      );
      break;
    case "clue_known":
      body = <Pick value={value.clue} options={choices.clues} onChange={(v) => set({ clue: v })} />;
      break;
    case "relation":
      body = (
        <>
          <Pick value={value.a} options={choices.people} allowParty onChange={(v) => set({ a: v })} />
          <Pick value={value.b} options={choices.people} allowParty onChange={(v) => set({ b: v })} />
          <Txt value={value.rel_type} onChange={(v) => set({ rel_type: v })} placeholder="关系类型" width={80} />
        </>
      );
      break;
    case "visited":
      body = (
        <>
          <Pick value={value.node} options={choices.plotNodes} onChange={(v) => set({ node: v })} />≥ <Num value={value.min_count} onChange={(v) => set({ min_count: v })} />次
        </>
      );
      break;
    case "check":
      body = (
        <>
          <Txt value={value.skill} onChange={(v) => set({ skill: v })} placeholder="技能" width={80} />
          DC <Num value={value.dc} onChange={(v) => set({ dc: v })} />
          <Txt value={value.dice} onChange={(v) => set({ dice: v })} width={60} />
        </>
      );
      break;
  }
  return (
    <div className={`cond depth-${Math.min(depth, 3)}`}>
      <div className="row wrap">
        {typeSelect}
        {value.type !== "all" && value.type !== "any" && value.type !== "not" && body}
      </div>
      {(value.type === "all" || value.type === "any" || value.type === "not") && body}
    </div>
  );
}

export function EffectsEditor({ value, onChange, choices }: { value: Effect[]; onChange: (e: Effect[]) => void; choices: Choices }) {
  const update = (i: number, e: Effect) => onChange(value.map((x, j) => (j === i ? e : x)));
  return (
    <div className="effects">
      {value.map((e, i) => {
        const set = (patch: Partial<Effect>) => update(i, { ...e, ...patch } as Effect);
        let fields: React.ReactNode = null;
        switch (e.type) {
          case "set_var":
            fields = (
              <>
                <Txt value={e.key} onChange={(v) => set({ key: v })} placeholder="变量名" />= <Txt value={JSON.stringify(e.value)} onChange={(v) => set({ value: parseValue(v) as never })} width={70} />
              </>
            );
            break;
          case "inc_var":
            fields = (
              <>
                <Txt value={e.key} onChange={(v) => set({ key: v })} placeholder="变量名" />+ <Num value={e.by} onChange={(v) => set({ by: v })} />
              </>
            );
            break;
          case "set_flag":
            fields = (
              <>
                <Txt value={e.name} onChange={(v) => set({ name: v })} placeholder="标志名" />
                <select value={String(e.value)} onChange={(x) => set({ value: x.target.value === "true" })}>
                  <option value="true">设置</option>
                  <option value="false">清除</option>
                </select>
              </>
            );
            break;
          case "set_quest_state":
            fields = (
              <>
                <Pick value={e.quest} options={choices.quests} onChange={(v) => set({ quest: v })} />
                <StateInput value={e.state} onChange={(v) => set({ state: v })} />
              </>
            );
            break;
          case "give_item":
          case "take_item":
            fields = (
              <>
                <Pick value={e.item} options={choices.items} onChange={(v) => set({ item: v })} />× <Num value={e.qty} onChange={(v) => set({ qty: v })} />
              </>
            );
            break;
          case "reveal_clue":
            fields = <Pick value={e.clue} options={choices.clues} onChange={(v) => set({ clue: v })} />;
            break;
          case "set_relation":
            fields = (
              <>
                <Pick value={e.a} options={choices.people} allowParty onChange={(v) => set({ a: v })} />
                <Pick value={e.b} options={choices.people} allowParty onChange={(v) => set({ b: v })} />
                <Txt value={e.rel_type} onChange={(v) => set({ rel_type: v })} width={80} />
                <select value={String(e.active)} onChange={(x) => set({ active: x.target.value === "true" })}>
                  <option value="true">建立</option>
                  <option value="false">解除</option>
                </select>
              </>
            );
            break;
          case "set_entity_state":
            fields = (
              <>
                <Pick value={e.entity} options={choices.people} onChange={(v) => set({ entity: v })} />
                <Txt value={e.field} onChange={(v) => set({ field: v })} width={80} />= <Txt value={JSON.stringify(e.value)} onChange={(v) => set({ value: parseValue(v) as never })} width={70} />
              </>
            );
            break;
          case "unlock_node":
            fields = <Pick value={e.node} options={choices.plotNodes} onChange={(v) => set({ node: v })} />;
            break;
          case "spawn":
            fields = <Txt value={e.changeset} onChange={(v) => set({ changeset: v })} placeholder="变更集 ID" />;
            break;
          case "adjust_mood":
            fields = (
              <>
                <Pick value={e.npc} options={choices.people} onChange={(v) => set({ npc: v })} />
                恐惧 <Num value={e.fear} onChange={(v) => set({ fear: v })} />
                愤怒 <Num value={e.anger} onChange={(v) => set({ anger: v })} />
                怀疑 <Num value={e.suspicion} onChange={(v) => set({ suspicion: v })} />
                压力 <Num value={e.stress} onChange={(v) => set({ stress: v })} />
                忠诚 <Num value={e.loyalty} onChange={(v) => set({ loyalty: v })} />
                <Txt value={e.reason} onChange={(v) => set({ reason: v })} width={160} placeholder="原因（必填）" />
              </>
            );
            break;
          case "upsert_record":
          case "delete_record":
            fields = <span className="hint">由程序管理</span>;
            break;
          case "adjust_bond":
            fields = (
              <>
                <Pick value={e.from} options={choices.people} allowParty onChange={(v) => set({ from: v })} />对
                <Pick value={e.to} options={choices.people} allowParty onChange={(v) => set({ to: v })} />
                强度 <Num value={e.strength} onChange={(v) => set({ strength: v })} />
                信任 <Num value={e.trust} onChange={(v) => set({ trust: v })} />
                好感 <Num value={e.affection} onChange={(v) => set({ affection: v })} />
                敌意 <Num value={e.hostility} onChange={(v) => set({ hostility: v })} />
                <Txt value={e.kind} onChange={(v) => set({ kind: v })} width={70} placeholder="关系类型" />
                <Txt value={e.reason} onChange={(v) => set({ reason: v })} width={160} placeholder="原因（必填）" />
                <label className="check">
                  <input type="checkbox" checked={e.mutual} onChange={(x) => set({ mutual: x.target.checked })} />
                  双方
                </label>
              </>
            );
            break;
        }
        return (
          <div key={i} className="row wrap effect">
            <select value={e.type} onChange={(x) => update(i, defaultEffect(x.target.value as EffType))}>
              {Object.entries(EFF_LABEL).filter(([k]) => !PROGRAM_ONLY_EFFECTS.includes(k as EffType)).map(([k, v]) => (
                <option key={k} value={k}>
                  {v}
                </option>
              ))}
            </select>
            {fields}
            <button className="link danger" onClick={() => onChange(value.filter((_, j) => j !== i))}>
              ×
            </button>
          </div>
        );
      })}
      <button className="link" onClick={() => onChange([...value, defaultEffect("set_flag")])}>
        + 添加效果
      </button>
    </div>
  );
}
