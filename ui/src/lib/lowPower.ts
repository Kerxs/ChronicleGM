// 低性能模式（全局设置，存在资料库 ui.low_power）：关掉着色器玻璃球、3D 骰子、粒子与大部分过渡动画。
// 改动后广播给所有用到它的组件；系统设置了"减少动态效果"时默认打开。
import { useEffect, useState } from "react";
import { api } from "../api";

const EVT = "chronicle-low-power";
let cached: boolean | null = null;

function systemPrefersReduced(): boolean {
  try {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false;
  }
}

export async function loadLowPower(): Promise<boolean> {
  if (cached !== null) return cached;
  try {
    const v = await api.uiPref("ui.low_power");
    cached = v === null ? systemPrefersReduced() : v === "1";
  } catch {
    cached = systemPrefersReduced();
  }
  document.documentElement.classList.toggle("low-power", cached);
  return cached;
}

export async function setLowPower(on: boolean): Promise<void> {
  cached = on;
  document.documentElement.classList.toggle("low-power", on);
  window.dispatchEvent(new CustomEvent(EVT, { detail: on }));
  await api.setUiPref("ui.low_power", on ? "1" : "0");
}

export function useLowPower(): boolean {
  const [on, setOn] = useState(cached ?? false);
  useEffect(() => {
    void loadLowPower().then(setOn);
    const f = (e: Event) => setOn((e as CustomEvent<boolean>).detail);
    window.addEventListener(EVT, f);
    return () => window.removeEventListener(EVT, f);
  }, []);
  return on;
}
