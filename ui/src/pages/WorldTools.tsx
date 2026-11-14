import { useEffect, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { api, formatTime } from "../api";
import type { Ctx } from "../App";
import type { Finding } from "../bindings/Finding";
import type { WorldVersion } from "../bindings/WorldVersion";
import type { ConflictStrategy } from "../bindings/ConflictStrategy";
import type { Node } from "../bindings/Node";
import type { BackupInfo } from "../bindings/BackupInfo";
import type { IntegrityReport } from "../bindings/IntegrityReport";
import { NODE_KIND } from "../lib/labels";

const BACKUP_KIND: Record<string, string> = {
  auto: "自动备份",
  migration: "升级前备份",
  before_restore: "回滚/还原前备份",
  before_repair: "修复前备份",
  other: "其他备份",
};

function formatSize(n: number): string {
  return n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`;
}

const FINDING: Record<string, string> = {
  relation_conflict: "关系矛盾",
  timeline_conflict: "时间线矛盾",
  duplicate_name: "重名",
  dangling_relation: "数据异常",
};

const STRATEGY: Record<ConflictStrategy, string> = {
  skip: "跳过已存在的内容",
  overwrite: "覆盖已存在的内容（锁定内容除外，可撤销）",
  copy: "作为副本导入（全部换新 ID）",
};

export default function WorldTools({ ctx, onOpenNode, onWorldImported }: { ctx: Ctx; onOpenNode: (id: string) => void; onWorldImported: (id: string) => void }) {
  const [findings, setFindings] = useState<Finding[] | null>(null);
  const [checking, setChecking] = useState(false);
  const [versions, setVersions] = useState<WorldVersion[]>([]);
  const [vname, setVname] = useState("");
  const [strategy, setStrategy] = useState<ConflictStrategy>("skip");
  const [trash, setTrash] = useState<Node[]>([]);
  const [counts, setCounts] = useState<[string, number][]>([]);
  const [backups, setBackups] = useState<BackupInfo[]>([]);
  const [integrity, setIntegrity] = useState<IntegrityReport | null>(null);
  const [repairing, setRepairing] = useState(false);

  useEffect(() => {
    api.listVersions(ctx.worldId).then(setVersions).catch((e: Error) => ctx.notify(e.message, "error"));
    api.listTrash(ctx.worldId).then(setTrash).catch(() => {});
    api.kindCounts(ctx.worldId).then(setCounts).catch(() => {});
    api.listBackups(ctx.worldId).then(setBackups).catch(() => {});
  }, [ctx.worldId, ctx.tick]);

  async function check() {
    setChecking(true);
    try {
      setFindings(await api.checkConsistency(ctx.worldId));
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    } finally {
      setChecking(false);
    }
  }

  async function snapshot() {
    try {
      await api.createVersion(ctx.worldId, vname || `版本 ${formatTime(Date.now())}`, "");
      setVname("");
      ctx.notify("已创建版本快照");
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  async function restore(v: WorldVersion) {
    if (!confirm(`回滚到「${v.name}」（${formatTime(v.created_at)}）？当前状态会先自动备份。`)) return;
    try {
      const backup = await api.restoreVersion(ctx.worldId, v.id);
      ctx.notify(`已回滚。回滚前的状态备份在：${backup}`);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  async function backupNow() {
    try {
      const b = await api.backupNow(ctx.worldId);
      ctx.notify(b ? `已备份：${b.file_name}` : "已备份");
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  async function restoreBackup(b: BackupInfo) {
    if (!confirm(`从备份「${b.file_name}」还原？当前状态会先另存为还原前备份，之后可以再还原回来。`)) return;
    try {
      const before = await api.restoreBackup(ctx.worldId, b.file_name);
      ctx.notify(`已还原。还原前的状态保存在：${before}`);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  async function checkIntegrity() {
    try {
      setIntegrity(await api.checkIntegrity(ctx.worldId));
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  async function repair() {
    if (!confirm("修复只重建索引、路径、排序等派生数据，并把父节点丢失的节点移到根级。修复前会先备份。继续？")) return;
    setRepairing(true);
    try {
      const r = await api.repairIntegrity(ctx.worldId);
      setIntegrity(r.after);
      ctx.notify(`修复完成：${r.actions.join("；") || "无需修改"}。修复前备份：${r.backup}`);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    } finally {
      setRepairing(false);
    }
  }

  async function exportWorld() {
    try {
      const path = await save({ filters: [{ name: "Chronicle GM 世界包", extensions: ["cgwpack"] }], defaultPath: "world.cgwpack" });
      if (!path) return;
      const m = await api.exportWorld(ctx.worldId, path, false);
      ctx.notify(`已导出 ${m.counts["nodes"] ?? 0} 个节点到 ${path}`);
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  async function pickBundle(): Promise<string | null> {
    const p = await open({ multiple: false, filters: [{ name: "Chronicle GM 世界包", extensions: ["cgwpack", "zip"] }] });
    return typeof p === "string" ? p : null;
  }

  async function importInto() {
    try {
      const path = await pickBundle();
      if (!path) return;
      const r = await api.importIntoWorld(ctx.worldId, path, strategy);
      ctx.notify(`导入完成：新增 ${r.inserted}，覆盖 ${r.overwritten}，跳过 ${r.skipped}，锁定未覆盖 ${r.protected}`);
      ctx.bump();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  async function importNew() {
    try {
      const path = await pickBundle();
      if (!path) return;
      const info = await api.importAsNewWorld(path, null);
      ctx.notify(`已导入为新世界「${info.name}」`);
      onWorldImported(info.id);
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    }
  }

  return (
    <section>
      <h2>世界工具</h2>
      {counts.length > 0 && (
        <p className="hint">
          节点统计：{counts.map(([k, n]) => `${NODE_KIND[k] ?? k} ${n}`).join("、")}
        </p>
      )}
      <div className="grid2">
        <div className="card">
          <h3>一致性检查</h3>
          <p className="hint">检查对立关系（同一时期既是盟友又是敌人）、人物死后/出生前参与事件、同类重名等。只读，不会修改内容。</p>
          <button className="primary" disabled={checking} onClick={() => void check()}>
            {checking ? "检查中…" : "检查整个世界"}
          </button>
          {findings && findings.length === 0 && <p className="ok-text">没有发现问题。</p>}
          {findings && findings.length > 0 && (
            <ul className="issues">
              {findings.map((f, i) => (
                <li key={i} className={f.severity}>
                  <strong>{FINDING[f.code] ?? f.code}</strong>：{f.message}{" "}
                  {f.node_ids.slice(0, 3).map((id) => (
                    <button key={id} className="link" onClick={() => onOpenNode(id)}>
                      查看
                    </button>
                  ))}
                </li>
              ))}
            </ul>
          )}
        </div>

        <div className="card">
          <h3>版本快照</h3>
          <div className="row">
            <input value={vname} onChange={(e) => setVname(e.target.value)} placeholder="版本名称（可选）" style={{ flex: 1 }} />
            <button onClick={() => void snapshot()}>创建快照</button>
          </div>
          <ul className="list">
            {versions.map((v) => (
              <li key={v.id} className="row">
                <span style={{ flex: 1 }}>
                  {v.name} <small className="hint">{formatTime(v.created_at)}</small>
                </span>
                <button onClick={() => void restore(v)}>回滚到此版本</button>
              </li>
            ))}
          </ul>
          {versions.length === 0 && <p className="hint">还没有快照。</p>}
        </div>

        <div className="card">
          <h3>备份与完整性</h3>
          <p className="hint">打开世界时，距上次自动备份超过一天会自动备份一次，保留最近 10 份；升级前、还原前、修复前的备份不会被自动删除。</p>
          <div className="row wrap">
            <button onClick={() => void backupNow()}>立即备份</button>
            <button onClick={() => void checkIntegrity()}>检查数据完整性</button>
            {integrity && integrity.issues.some((i) => i.repairable) && (
              <button className="primary" disabled={repairing} onClick={() => void repair()}>
                {repairing ? "修复中…" : "安全修复"}
              </button>
            )}
          </div>
          {integrity && integrity.ok && <p className="ok-text">数据完整，没有发现问题。</p>}
          {integrity && !integrity.ok && (
            <ul className="issues">
              {integrity.issues.map((i, k) => (
                <li key={k} className={i.repairable ? "warning" : "error"}>
                  {i.message}
                  {!i.repairable && "（无法自动修复，建议从下方备份还原）"}
                </li>
              ))}
            </ul>
          )}
          <ul className="list">
            {backups.slice(0, 20).map((b) => (
              <li key={b.file_name} className="row">
                <span style={{ flex: 1 }}>
                  <span className="kind">{BACKUP_KIND[b.kind] ?? b.kind}</span>
                  {formatTime(b.created_at)} <small className="hint">{formatSize(b.size)}</small>
                </span>
                <button onClick={() => void restoreBackup(b)}>还原</button>
              </li>
            ))}
          </ul>
          {backups.length === 0 && <p className="hint">还没有备份。</p>}
        </div>

        <div className="card">
          <h3>导入导出</h3>
          <div className="row wrap">
            <button onClick={() => void exportWorld()}>导出当前世界…</button>
            <button onClick={() => void importNew()}>导入为新世界…</button>
          </div>
          <div className="row" style={{ marginTop: 8 }}>
            <select value={strategy} onChange={(e) => setStrategy(e.target.value as ConflictStrategy)}>
              {Object.entries(STRATEGY).map(([k, v]) => (
                <option key={k} value={k}>
                  {v}
                </option>
              ))}
            </select>
            <button onClick={() => void importInto()}>合并到当前世界…</button>
          </div>
          <p className="hint">导出包含世界树、关系、剧本与剧情图，不含战役进度。导入在一个事务中完成，失败不会留下部分内容，成功后可在历史中撤销。</p>
        </div>

        <div className="card">
          <h3>回收站（{trash.length}）</h3>
          <ul className="list">
            {trash.map((n) => (
              <li key={n.id} className="row">
                <span style={{ flex: 1 }}>
                  <span className="kind">{NODE_KIND[n.kind]}</span>
                  {n.title} <small className="hint">{formatTime(n.deleted_at)}</small>
                </span>
                <button
                  onClick={() =>
                    void api
                      .restoreNode(ctx.worldId, n.id)
                      .then(() => {
                        ctx.notify(`已恢复「${n.title}」`);
                        ctx.bump();
                      })
                      .catch((e: Error) => ctx.notify(e.message, "error"))
                  }
                >
                  恢复
                </button>
              </li>
            ))}
          </ul>
        </div>
      </div>
    </section>
  );
}
