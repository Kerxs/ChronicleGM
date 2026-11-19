// 界面大小（设置 → 通用 → 外观）：整页缩放。
export function applyZoom(v: string | null | undefined) {
  const z = Number(v);
  document.documentElement.style.zoom = z >= 0.75 && z <= 1.5 ? String(z) : "";
}
