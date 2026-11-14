// 使用引导：第一次打开（还没有世界）时显示，也可以从标题栏的世界菜单再次打开。
// 四步：认识软件 → 连接模型（实时检查）→ 创建第一个世界 → 常用位置。
import { useEffect, useState } from "react";
import { api, type ProviderView } from "../api";
import type { DiscoveredService } from "../bindings/DiscoveredService";
import type { WorldInfo } from "../bindings/WorldInfo";
import Icon from "./Icon";

const STEPS = ["欢迎", "连接模型", "创建世界", "开始"];

export default function Onboarding({
  worlds,
  onCreateWorld,
  onSelectWorld,
  onOpenModels,
  onClose,
}: {
  worlds: WorldInfo[];
  onCreateWorld: (name: string) => Promise<boolean>;
  onSelectWorld: (id: string) => void;
  onOpenModels: () => void;
  onClose: () => void;
}) {
  const [step, setStep] = useState(0);
  const [dir, setDir] = useState<1 | -1>(1);
  const go = (n: number) => {
    setDir(n > step ? 1 : -1);
    setStep(n);
  };

  return (
    <div className="onb-backdrop" role="dialog" aria-label="使用引导">
      <div className="onb-card">
        <div className="onb-top">
          <div className="onb-dots">
            {STEPS.map((s, i) => (
              <button key={s} className={`onb-dot${i === step ? " active" : i < step ? " done" : ""}`} onClick={() => go(i)} title={s} aria-label={s} />
            ))}
          </div>
          <button className="link onb-skip" onClick={onClose}>
            跳过引导
          </button>
        </div>
        <div key={step} className={`onb-body ${dir > 0 ? "from-right" : "from-left"}`}>
          {step === 0 && <Welcome />}
          {step === 1 && <Models onOpenModels={onOpenModels} />}
          {step === 2 && <CreateWorld worlds={worlds} onCreateWorld={onCreateWorld} onSelectWorld={onSelectWorld} onDone={() => go(3)} />}
          {step === 3 && <Finish />}
        </div>
        <div className="onb-foot">
          {step > 0 ? <button onClick={() => go(step - 1)}>上一步</button> : <span />}
          {step < STEPS.length - 1 ? (
            <button className="primary" onClick={() => go(step + 1)}>
              下一步
            </button>
          ) : (
            <button className="primary" onClick={onClose}>
              开始使用
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

function Welcome() {
  const items: [string, string, string][] = [
    ["tree", "世界与剧本", "一句话生成世界树、剧本大纲与剧情分支；每处修改都先进审阅台，你确认后才写入。"],
    ["game", "AI 主持", "GM 讲述、NPC 各自说话；骰子、规则和状态变化都由程序判定，AI 不能私自改数据。"],
    ["image", "图片与语音", "节点配图、看图生成设定，叙述可以朗读；游戏里会显示此刻的画面。"],
  ];
  return (
    <>
      <div className="onb-hero">
        <img className="app-icon-hero" src="/app-icon.png" alt="Chronicle GM" draggable={false} />
        <h2>欢迎使用 Chronicle GM</h2>
        <p className="hint">本地 AI 跑团主持工具。模型由你自己准备，数据都保存在这台电脑上。</p>
      </div>
      <div className="onb-features">
        {items.map(([icon, t, d], i) => (
          <div key={t} className="onb-feature" style={{ animationDelay: `${0.08 + i * 0.07}s` }}>
            <span className="onb-ico"><Icon name={icon} /></span>
            <strong>{t}</strong>
            <p>{d}</p>
          </div>
        ))}
      </div>
    </>
  );
}

function Models({ onOpenModels }: { onOpenModels: () => void }) {
  const [providers, setProviders] = useState<ProviderView[] | null>(null);
  const [found, setFound] = useState<DiscoveredService[] | null>(null);
  const [scanning, setScanning] = useState(false);
  useEffect(() => {
    api.listProviders().then(setProviders).catch(() => setProviders([]));
  }, []);
  const chat = providers?.filter((p) => p.config.server_kind !== "breeze_tts") ?? [];
  const tts = providers?.filter((p) => p.config.server_kind === "breeze_tts") ?? [];
  const tested = chat.find((p) => p.config.capabilities?.chat);
  const vision = chat.some((p) => p.config.capabilities?.vision);
  const row = (ok: boolean | null, title: string, detail: string) => (
    <li className={`onb-check ${ok === null ? "wait" : ok ? "ok" : "no"}`}>
      <span className="onb-mark">{ok === null ? "…" : ok ? "✓" : "—"}</span>
      <div>
        <strong>{title}</strong>
        <small className="hint">{detail}</small>
      </div>
    </li>
  );
  return (
    <>
      <h2>连接模型</h2>
      <p className="hint">软件不下载、不安装模型。先在本机启动你的模型服务（如 llama-server、Ollama、LM Studio），这里会自动识别。</p>
      <ul className="onb-checks">
        {row(providers === null ? null : chat.length > 0, "语言模型", chat.length ? `已配置 ${chat.length} 个：${chat.map((p) => p.config.name).join("、")}` : "还没有配置（GM、生成世界与剧本都需要它）")}
        {row(providers === null ? null : !!tested, "能力测试", tested ? `「${tested.config.name}」对话✓${tested.config.capabilities?.json_schema ? " 结构化输出✓" : ""}` : "配置后在「模型服务」里点「重新测试」")}
        {row(providers === null ? null : vision, "看图（可选）", vision ? "能看图：可以用图片生成设定" : "需要带视觉模块的模型")}
        {row(providers === null ? null : tts.length > 0, "语音朗读（可选）", tts.length ? `已配置：${tts.map((p) => p.config.name).join("、")}` : "没有语音服务时不朗读，其他功能不受影响")}
      </ul>
      <div className="row wrap">
        <button
          disabled={scanning}
          onClick={() => {
            setScanning(true);
            api
              .discoverServices([])
              .then(setFound)
              .catch(() => setFound([]))
              .finally(() => setScanning(false));
          }}
        >
          {scanning ? "正在扫描本机…" : "扫描本机服务"}
        </button>
        <button className="link" onClick={onOpenModels}>
          打开「模型服务」详细设置
        </button>
      </div>
      {found && (
        <p className="hint onb-found">
          {found.length === 0
            ? "没有发现正在运行的服务。请先启动模型程序，或在「模型服务」里手动添加地址。"
            : `发现 ${found.length} 个服务：${found.map((f) => `${f.base_url}（${f.models.map((m) => m.id).join("、") || f.kind}）`).join("；")}。到「模型服务」里一键添加。`}
        </p>
      )}
    </>
  );
}

function CreateWorld({ worlds, onCreateWorld, onSelectWorld, onDone }: { worlds: WorldInfo[]; onCreateWorld: (n: string) => Promise<boolean>; onSelectWorld: (id: string) => void; onDone: () => void }) {
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  return (
    <>
      <h2>创建第一个世界</h2>
      <p className="hint">一个世界是一份独立的存档：世界树、剧本、战役都在里面。之后可以在标题栏的世界菜单里随时切换或新建。</p>
      <form
        className="onb-create"
        onSubmit={(e) => {
          e.preventDefault();
          if (!name.trim()) return;
          setBusy(true);
          void onCreateWorld(name.trim())
            .then((ok) => ok && onDone())
            .finally(() => setBusy(false));
        }}
      >
        <input autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="世界名称，例如：雾港编年史" maxLength={100} />
        <button className="primary" type="submit" disabled={!name.trim() || busy}>
          {busy ? "正在创建…" : "创建"}
        </button>
      </form>
      <p className="hint">创建后可以在「世界树」里用「一句话生成世界」让 AI 起草顶层设定，也可以手动添加。</p>
      {worlds.length > 0 && (
        <div className="onb-existing">
          <div className="hint">或者打开已有的世界：</div>
          <div className="row wrap">
            {worlds.slice(0, 6).map((w) => (
              <button
                key={w.id}
                className="chip"
                onClick={() => {
                  onSelectWorld(w.id);
                  onDone();
                }}
              >
                {w.name}
              </button>
            ))}
          </div>
        </div>
      )}
    </>
  );
}

function Finish() {
  const tips: [string, string, string][] = [
    ["tree", "世界树", "设定的层级、配图与关系"],
    ["script", "剧本", "大纲、场景与剧情分支"],
    ["review", "审阅与任务", "确认 AI 的修改、查看生成进度"],
    ["game", "游戏", "建立战役，开始跑团"],
    ["models", "模型与性能", "模型服务、启动参数与资源"],
  ];
  return (
    <>
      <h2>准备好了</h2>
      <p className="hint">左侧导航按「冒险 / 创作 / 系统」分组，可以点底部的箭头收起成图标。常用的位置：</p>
      <ul className="onb-tips">
        {tips.map(([icon, t, d], i) => (
          <li key={t} style={{ animationDelay: `${0.05 + i * 0.05}s` }}>
            <span className="onb-ico"><Icon name={icon} /></span>
            <strong>{t}</strong>
            <span className="hint">{d}</span>
          </li>
        ))}
      </ul>
      <p className="hint">引导可以随时从标题栏的世界菜单里再次打开。</p>
    </>
  );
}
