// 界面与数据：缩略图大小；数据目录（迁移：复制 → 校验 → 重启后切换，旧目录保留）。
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { api } from "../api";
import { THUMB_SIZES, applyThumbSize } from "../lib/thumbs";

type DirInfo = { current: string; default: string; configured: string | null; warning: string | null; env_override: boolean };

export default function AppearanceData({ notify }: { notify: (m: string, k?: "info" | "error") => void }) {
  const [size, setSize] = useState("m");
  const [dir, setDir] = useState<DirInfo | null>(null);
  const [target, setTarget] = useState("");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<string | null>(null);
  const [media, setMedia] = useState<{ scene_images: boolean; portraits: boolean; pre_voice: boolean } | null>(null);
  useEffect(() => {
    api.autoMediaSettings().then(setMedia).catch(() => setMedia(null));
  }, []);
  const saveMedia = (m: { scene_images: boolean; portraits: boolean; pre_voice: boolean }) => {
    setMedia(m);
    void api.setAutoMediaSettings(m).catch((e: Error) => notify(e.message, "error"));
  };
  useEffect(() => {
    api.uiPref("ui.thumb_size").then((v) => setSize(v || "m")).catch(() => {});
    api.dataDirInfo().then(setDir).catch(() => setDir(null));
  }, []);

  async function migrate() {
    if (!target.trim()) return;
    if (!window.confirm(`把全部数据（世界、战役、备份、设置）复制到：\n${target}\n\n复制并校验完成后，重启程序就会使用新位置。旧目录会原样保留，确认无误后再自己删除。继续？`)) return;
    setBusy(true);
    try {
      const r = await api.migrateDataDir(target.trim());
      setDone(r.target);
      notify(`已复制 ${r.files} 个文件（${(r.bytes / 1048576).toFixed(1)} MB，${r.databases} 个数据库校验通过）。重启程序后生效`);
    } catch (e) {
      notify((e as Error).message, "error");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="card">
      <h3>界面与数据</h3>
      <label className="check">
        缩略图大小
        <div className="seg-mini">
          {THUMB_SIZES.map(([k, label]) => (
            <button
              key={k}
              className={size === k ? "on" : ""}
              onClick={() => {
                setSize(k);
                applyThumbSize(k);
                void api.setUiPref("ui.thumb_size", k).catch(() => {});
              }}
            >
              {label}
            </button>
          ))}
        </div>
      </label>
      {media && (
        <div className="auto-media">
          <strong>后台自动生成（默认关闭；需要先在「模型服务」里配置生图 / 语音服务，云端服务可能产生费用）</strong>
          <label className="check">
            <input type="checkbox" checked={media.scene_images} onChange={(e) => saveMedia({ ...media, scene_images: e.target.checked })} />
            进入新地点时建议生成场景插图（也建议最近一个能去的地点；你确认后才生成、看过预览才保存）
          </label>
          <label className="check">
            <input type="checkbox" checked={media.portraits} onChange={(e) => saveMedia({ ...media, portraits: e.target.checked })} />
            人物第一次出场时建议生成立绘（每回合最多一个建议；同样需要你确认）
          </label>
          <label className="check">
            <input type="checkbox" checked={media.pre_voice} onChange={(e) => saveMedia({ ...media, pre_voice: e.target.checked })} />
            每回合先在后台合成叙述语音（点朗读时不用等）
          </label>
          <p className="hint">只把玩家可见的描述发给生图服务，不发送秘密；已经有图片的节点不会重复生成。</p>
        </div>
      )}
      {dir && (
        <div className="datadir">
          <div>
            数据目录：<code>{dir.current}</code>
            {dir.configured && dir.configured !== dir.current && <span className="warn-text">（设置的位置：{dir.configured}）</span>}
          </div>
          {dir.warning && <p className="warn-text">{dir.warning}</p>}
          {done && <p className="ok-text">已复制到 {done}，重启程序后生效。</p>}
          {dir.env_override ? (
            <p className="hint">现在用环境变量 CHRONICLE_DATA_DIR 指定了数据目录（开发 / 测试），不能在这里迁移。</p>
          ) : (
            <div className="row wrap">
              <input value={target} onChange={(e) => setTarget(e.target.value)} placeholder="新的数据目录（选一个空文件夹）" style={{ flex: 1, minWidth: 240 }} />
              <button
                onClick={() =>
                  void open({ directory: true, multiple: false, title: "选择新的数据目录（空文件夹）" })
                    .then((p) => typeof p === "string" && setTarget(p))
                    .catch(() => {})
                }
              >
                选择…
              </button>
              <button className="primary" disabled={busy || !target.trim()} onClick={() => void migrate()}>
                {busy ? "正在复制并校验…" : "迁移到这里"}
              </button>
              {dir.configured && (
                <button
                  className="link"
                  title="下次启动使用默认目录（不复制数据）"
                  onClick={() =>
                    void api
                      .resetDataDir()
                      .then(() => notify(`重启后使用默认目录：${dir.default}`))
                      .catch((e: Error) => notify(e.message, "error"))
                  }
                >
                  恢复默认位置
                </button>
              )}
            </div>
          )}
          <p className="hint">目标必须是空文件夹并且能写入；迁移时数据库按一致的快照复制，并逐个校验。迁移期间不要关闭程序。</p>
        </div>
      )}
    </div>
  );
}
