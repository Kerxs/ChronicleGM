// 设置页「模型启动参数」：列出托管的模型程序，可视化编辑启动参数（与性能中心是同一份配置）。
import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import type { ModelProfile } from "../bindings/ModelProfile";
import type { ProcStatus } from "../bindings/ProcStatus";
import { ProfileForm, emptyProfile } from "../pages/Perf";
import { cache_ram_label } from "../lib/launchSummary";

const ROLE: Record<string, string> = { llm: "大语言模型", tts: "语音合成", asr: "语音识别" };

export default function LaunchSettingsCard({ notify }: { notify: (m: string, k?: "info" | "error") => void }) {
  const [list, setList] = useState<ProcStatus[]>([]);
  const [editing, setEditing] = useState<ModelProfile | null>(null);
  const refresh = useCallback(() => {
    api.listManagedModels().then(setList).catch((e: Error) => notify(e.message, "error"));
  }, [notify]);
  useEffect(refresh, [refresh]);

  return (
    <div className="card launch-card">
      <h3>模型启动参数（托管模式）</h3>
      <p className="hint">
        软件按这里的程序路径和参数启动、停止你的模型程序（默认不托管，只识别已经在运行的服务）。可以从你的 .bat 导入，再用下面的选项调整；改动在下次启动模型时生效，不会改你的 .bat 文件。
      </p>
      {list.length === 0 && <p className="hint">还没有托管的模型程序。</p>}
      <ul className="launch-list">
        {list.map((m) => (
          <li key={m.profile.id} className="row">
            <span className="kind">{ROLE[m.profile.role] ?? m.profile.role}</span>
            <strong>{m.profile.name}</strong>
            <small className="hint">
              端口 {m.profile.port}
              {m.profile.role === "llm" && ` · ${cache_ram_label(m.profile.args)}`} · {m.profile.args.length} 个参数
            </small>
            <span className="spacer" />
            <button className="small" onClick={() => setEditing({ ...m.profile })}>
              编辑启动参数
            </button>
            <button
              className="small danger"
              onClick={() => {
                if (window.confirm(`删除托管配置「${m.profile.name}」？（只删除配置，不会删除模型文件）`))
                  void api.deleteManagedModel(m.profile.id).then(refresh).catch((e: Error) => notify(e.message, "error"));
              }}
            >
              删除
            </button>
          </li>
        ))}
      </ul>
      {!editing && (
        <button onClick={() => setEditing(emptyProfile())}>添加模型程序（可从 .bat 导入）</button>
      )}
      {editing && (
        <ProfileForm
          profile={editing}
          notify={notify}
          onCancel={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            notify("已保存，下次启动模型时生效");
            refresh();
          }}
        />
      )}
    </div>
  );
}
