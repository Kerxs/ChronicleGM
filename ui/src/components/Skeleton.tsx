// 骨架屏：页面代码或数据还没到时，先显示与真实布局相近的灰色块（带微光扫过），不再显示"加载中…"文字。
export function PageSkeleton() {
  return (
    <div className="skeleton-page" aria-busy="true" aria-label="正在加载">
      <div className="sk sk-title" />
      <div className="sk-row">
        <div className="sk sk-chip" />
        <div className="sk sk-chip" />
        <div className="sk sk-chip short" />
      </div>
      <div className="sk-cards">
        {[0, 1, 2].map((i) => (
          <div key={i} className="sk-card" style={{ animationDelay: `${i * 0.06}s` }}>
            <div className="sk sk-line w60" />
            <div className="sk sk-line" />
            <div className="sk sk-line w80" />
          </div>
        ))}
      </div>
    </div>
  );
}

/** 小块骨架（图片、行内容）。 */
export function SkeletonBlock({ w = "100%", h = 14, radius = 8 }: { w?: number | string; h?: number | string; radius?: number }) {
  return <span className="sk" style={{ display: "block", width: w, height: h, borderRadius: radius }} />;
}

/** 面板骨架（游戏页右侧各页签、节点详情）。 */
export function PanelSkeleton() {
  return (
    <div className="sk-card" aria-busy="true" aria-label="正在加载">
      <div className="sk sk-line w60" />
      <div className="sk sk-line" />
      <div className="sk sk-line w80" />
      <div className="sk sk-line" />
    </div>
  );
}
