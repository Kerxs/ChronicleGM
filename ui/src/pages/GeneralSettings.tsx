// 设置 → 通用：启动与后台、外观、游戏、更新、提示词、数据。每一项改了立即生效、自动保存。
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { api } from "../api";
import { useLowPower, setLowPower } from "../lib/lowPower";
import { checkUpdates, useUpdates } from "../lib/updates";
import { applyZoom } from "../lib/zoom";

type Notify = (m: string, k?: "info" | "error") => void;
type Sys = { close_to_tray: boolean; autostart: boolean; start_minimized: boolean; data_dir: string };
type PromptInfo = { version: string; builtin: number; pack: number | null; pack_date: string | null; overridden: string[]; auto_check: boolean };

export default function GeneralSettings({ notify }: { notify: Notify }) {
  const [sys, setSys] = useState<Sys | null>(null);
  const [startPage, setStartPage] = useState("lobby");
  const [zoom, setZoom] = useState("1");
  const [autoUpdate, setAutoUpdate] = useState(true);
  const [dice, setDice] = useState<"auto" | "click" | "physical">("auto");
  const [autoRead, setAutoRead] = useState(false);
  const [prompt, setPrompt] = useState<PromptInfo | null>(null);
  const [promptBusy, setPromptBusy] = useState(false);
  const lowPower = useLowPower();
  const updates = useUpdates();
  const err = (e: unknown) => notify(e instanceof Error ? e.message : String(e), "error");

  const loadPrompt = () => invoke<PromptInfo>("prompt_info").then(setPrompt).catch(() => {});
  useEffect(() => {
    invoke<Sys>("sys_settings").then(setSys).catch(err);
    api.uiPref("ui.start_page").then((v) => setStartPage(v ?? "lobby")).catch(() => {});
    api.uiPref("ui.zoom").then((v) => setZoom(v ?? "1")).catch(() => {});
    api.uiPref("ui.auto_update").then((v) => setAutoUpdate(v !== "0")).catch(() => {});
    api.diceMode().then(setDice).catch(() => {});
    try {
      setAutoRead(localStorage.getItem("autoRead") === "1");
    } catch {
      /* 忽略 */
    }
    void loadPrompt();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const pref = (k: string, v: string) => api.setUiPref(k, v).catch(err);
  const setTray = (on: boolean) =>
    invoke("sys_set_close_to_tray", { on })
      .then(() => setSys((s) => (s ? { ...s, close_to_tray: on } : s)))
      .catch(err);
  const setAuto = (on: boolean, minimized: boolean) =>
    invoke("sys_set_autostart", { on, minimized })
      .then(() => setSys((s) => (s ? { ...s, autostart: on, start_minimized: on && minimized } : s)))
      .catch(err);
  const open = (kind: string) => invoke("sys_open_folder", { kind }).catch(err);
  const checkPrompt = async () => {
    setPromptBusy(true);
    try {
      const r = await invoke<{ updated: boolean; version: number; message: string }>("prompt_update_check");
      notify(r.message);
      await loadPrompt();
    } catch (e) {
      err(e);
    } finally {
      setPromptBusy(false);
    }
  };

  return (
    <div className="gs-page">
      <section className="gs-card">
        <h3>启动与后台</h3>
        <Row label="关闭窗口时" hint="转到后台时，下载、联机房间和本地模型会继续运行；从任务栏右下角的托盘图标打开或退出。">
          <select value={sys?.close_to_tray ? "tray" : "quit"} onChange={(e) => void setTray(e.target.value === "tray")}>
            <option value="quit">退出软件</option>
            <option value="tray">转到后台（托盘）</option>
          </select>
        </Row>
        <Row label="开机自动启动">
          <input type="checkbox" checked={!!sys?.autostart} onChange={(e) => void setAuto(e.target.checked, !!sys?.start_minimized)} />
        </Row>
        <Row label="开机后直接在后台" hint="不弹出窗口，只在托盘里（需要同时选「关闭窗口时 → 转到后台」）。">
          <input type="checkbox" disabled={!sys?.autostart} checked={!!sys?.start_minimized} onChange={(e) => void setAuto(true, e.target.checked)} />
        </Row>
        <Row label="打开软件时">
          <select
            value={startPage}
            onChange={(e) => {
              setStartPage(e.target.value);
              void pref("ui.start_page", e.target.value);
            }}
          >
            <option value="lobby">显示大厅</option>
            <option value="continue">继续上次的游戏</option>
          </select>
        </Row>
        <div className="gs-actions">
          <button className="danger" onClick={() => window.confirm("退出 Chronicle GM？正在运行的本地模型和下载会停止（下载下次会继续）。") && void invoke("sys_quit")}>
            退出软件
          </button>
        </div>
      </section>

      <section className="gs-card">
        <h3>外观</h3>
        <Row label="界面大小">
          <select
            value={zoom}
            onChange={(e) => {
              setZoom(e.target.value);
              applyZoom(e.target.value);
              void pref("ui.zoom", e.target.value);
            }}
          >
            <option value="0.9">小（90%）</option>
            <option value="1">标准（100%）</option>
            <option value="1.1">大（110%）</option>
            <option value="1.25">特大（125%）</option>
          </select>
        </Row>
        <Row label="低性能模式" hint="关掉着色器玻璃球、3D 骰子和大部分动画。老电脑或用电池时打开。">
          <input type="checkbox" checked={lowPower} onChange={(e) => void setLowPower(e.target.checked).catch(err)} />
        </Row>
      </section>

      <section className="gs-card">
        <h3>游戏</h3>
        <Row label="掷骰">
          <select
            value={dice}
            onChange={(e) => {
              const m = e.target.value as typeof dice;
              void api.setDiceMode(m).then(() => setDice(m)).catch(err);
            }}
          >
            <option value="auto">程序自动掷</option>
            <option value="click">我来点击掷骰</option>
            <option value="physical">掷实体骰子，输入点数</option>
          </select>
        </Row>
        <Row label="自动朗读 GM 的讲述" hint="需要在「模型服务」里配置语音服务。">
          <input
            type="checkbox"
            checked={autoRead}
            onChange={(e) => {
              setAutoRead(e.target.checked);
              try {
                localStorage.setItem("autoRead", e.target.checked ? "1" : "0");
              } catch {
                /* 忽略 */
              }
            }}
          />
        </Row>
      </section>

      <section className="gs-card">
        <h3>更新</h3>
        <Row label="自动检查软件更新" hint="打开软件时检查一次，有新版本只提示，不会自动安装。">
          <input
            type="checkbox"
            checked={autoUpdate}
            onChange={(e) => {
              setAutoUpdate(e.target.checked);
              void pref("ui.auto_update", e.target.checked ? "1" : "0");
            }}
          />
        </Row>
        <div className="gs-actions">
          <button disabled={updates.checking} onClick={() => void checkUpdates(true).then((i) => !i && notify("软件已经是最新版本"))}>
            {updates.checking ? "检查中…" : "检查软件更新"}
          </button>
        </div>
        <Row
          label="提示词"
          hint={
            prompt
              ? `正在使用 ${prompt.version}${prompt.pack ? `（在线更新的 v${prompt.pack}${prompt.pack_date ? `，${prompt.pack_date}` : ""}）` : "（软件自带）"}${prompt.overridden.length ? `；${prompt.overridden.length} 个模板用了你自己的版本` : ""}。提示词可以单独更新，不用更新软件。`
              : ""
          }
        >
          <span />
        </Row>
        <Row label="自动检查提示词更新">
          <input type="checkbox" checked={!!prompt?.auto_check} onChange={(e) => void invoke("prompt_set_auto", { on: e.target.checked }).then(loadPrompt).catch(err)} />
        </Row>
        <div className="gs-actions">
          <button disabled={promptBusy} onClick={() => void checkPrompt()}>
            {promptBusy ? "检查中…" : "检查提示词更新"}
          </button>
          {prompt?.pack != null && (
            <button onClick={() => window.confirm("不用在线更新的提示词，改回软件自带的版本？") && void invoke("prompt_reset").then(() => { notify("已改回软件自带的提示词"); void loadPrompt(); }).catch(err)}>
              改回自带版本
            </button>
          )}
          <button className="link" onClick={() => void open("prompts")}>
            打开自定义提示词文件夹
          </button>
        </div>
      </section>

      <section className="gs-card">
        <h3>数据</h3>
        <Row label="数据位置" hint={sys?.data_dir}>
          <button onClick={() => void open("data")}>打开</button>
        </Row>
        <Row label="日志" hint="遇到问题时把日志发给开发者。">
          <button onClick={() => void open("logs")}>打开日志文件夹</button>
        </Row>
        <p className="hint">要把数据搬到别的盘，在「模型服务」页的「关于」里迁移。</p>
      </section>
    </div>
  );
}

function Row({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="gs-row">
      <div>
        <div className="gs-label">{label}</div>
        {hint && <div className="hint gs-hint">{hint}</div>}
      </div>
      <div className="gs-ctl">{children}</div>
    </div>
  );
}
