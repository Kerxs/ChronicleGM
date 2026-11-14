// 更新对话框：新版本号、更新说明、下载进度。安装包下载后先校验签名，通过才安装。
import { closeUpdate, installUpdate, useUpdates } from "../lib/updates";

const mb = (b: number) => `${(b / 1048576).toFixed(1)} MB`;

export default function UpdateDialog() {
  const u = useUpdates();
  if (!u.open || !u.info) return null;
  const pct = u.total ? Math.min(100, (u.downloaded / u.total) * 100) : null;
  return (
    <div className="onb-backdrop" role="dialog" aria-label="软件更新" onClick={closeUpdate}>
      <div className="update-card" onClick={(e) => e.stopPropagation()}>
        <div className="update-head">
          <img className="app-icon-small" src="/app-icon.png" alt="" draggable={false} />
          <div>
            <h2>发现新版本 {u.info.version}</h2>
            <p className="hint">
              当前版本 {u.info.current}
              {u.info.date ? ` · 发布于 ${u.info.date.slice(0, 10)}` : ""}
            </p>
          </div>
        </div>
        {u.info.notes && <pre className="update-notes">{u.info.notes}</pre>}
        <p className="hint">更新会下载安装包并校验签名，然后关闭程序、安装、重新打开。你的世界、战役、设置都在数据目录里，更新不会动它们（自动备份照常保留）。托管的模型会先停止。</p>
        {u.installing && (
          <div className="update-progress">
            <div className="bar">
              <span style={{ width: pct == null ? "35%" : `${pct}%` }} className={pct == null ? "indeterminate" : ""} />
            </div>
            <small className="hint">
              {u.downloaded > 0 ? `已下载 ${mb(u.downloaded)}${u.total ? ` / ${mb(u.total)}` : ""}` : "正在连接…"}
              {pct === 100 && " · 校验签名并安装…"}
            </small>
          </div>
        )}
        {u.error && <p className="warn-text">{u.error}</p>}
        <div className="onb-foot">
          <button onClick={closeUpdate} disabled={u.installing}>
            稍后
          </button>
          <button className="primary" disabled={u.installing} onClick={() => void installUpdate()}>
            {u.installing ? "正在更新…" : "立即更新"}
          </button>
        </div>
      </div>
    </div>
  );
}
