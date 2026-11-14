// 更新状态（全局共享）：启动后自动检查一次（可以在「关于」里关闭），标题栏、关于卡片、更新对话框都读这里。
import { useSyncExternalStore } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";

export interface UpdateInfo {
  current: string;
  version: string;
  date: string | null;
  notes: string | null;
}

interface State {
  info: UpdateInfo | null;
  checking: boolean;
  error: string | null;
  checkedAt: number | null;
  /** 对话框是否打开。 */
  open: boolean;
  installing: boolean;
  downloaded: number;
  total: number | null;
}

let state: State = { info: null, checking: false, error: null, checkedAt: null, open: false, installing: false, downloaded: 0, total: null };
const subs = new Set<() => void>();
const set = (p: Partial<State>) => {
  state = { ...state, ...p };
  subs.forEach((f) => f());
};

export function useUpdates(): State {
  return useSyncExternalStore(
    (f) => {
      subs.add(f);
      return () => subs.delete(f);
    },
    () => state,
  );
}

/** 检查更新。`openIfFound`：发现新版本时打开对话框（手动检查时用）。 */
export async function checkUpdates(openIfFound = false): Promise<UpdateInfo | null> {
  if (state.checking) return state.info;
  set({ checking: true, error: null });
  try {
    const info = await api.updateCheck();
    set({ info, checking: false, checkedAt: Date.now(), open: openIfFound && !!info ? true : state.open });
    return info;
  } catch (e) {
    set({ checking: false, error: (e as Error).message, checkedAt: Date.now() });
    return null;
  }
}

export const openUpdate = () => set({ open: true });
export const closeUpdate = () => !state.installing && set({ open: false });

export async function installUpdate() {
  set({ installing: true, downloaded: 0, total: null, error: null });
  const un = await listen<{ downloaded: number; total: number | null }>("update-progress", (e) => set({ downloaded: e.payload.downloaded, total: e.payload.total }));
  try {
    // 成功时程序会自动关闭并运行安装程序，之后重新打开。
    await api.updateInstall();
  } catch (e) {
    set({ installing: false, error: (e as Error).message });
  } finally {
    un();
  }
}
