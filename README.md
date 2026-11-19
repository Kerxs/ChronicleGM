# Chronicle GM

**在自己电脑上运行的 AI 跑团主持人（TRPG GM）与世界创作工具。**

选一个剧本、挑一个角色，GM 会为你讲述开场、扮演 NPC、按规则掷骰判定，记住你做过的每一件事。也可以和朋友联机一起玩，或者自己写剧本、搭世界。

Windows 10 / 11 · Tauri 2 + Rust + React · 数据全部存在本机

[⬇️ 下载最新版](https://github.com/Kerxs/ChronicleGM/releases/latest) · [详细说明](docs/details.md) · [更新记录](https://github.com/Kerxs/ChronicleGM/releases)

---

## 功能

### 🎲 游戏
- **AI 主持**：GM 讲述剧情、扮演每个 NPC，NPC 有自己的性格、记忆和秘密；每回合给出可选行动，也可以随便写自己想做的事。
- **程序判定**：检定、战斗、伤害、物品和弹药都由程序结算，骰子有 3D 动画（也可以掷实体骰子再输入点数）。GM 只能按结果讲，不能替你改。
- **两个对话框**：大框是你和 NPC 的对话，小框是 GM 的叙述，中间的分隔条可以拖动调宽窄。
- **战斗**：按先攻轮流行动，有专门的战斗场景。
- **存档与多时间线**：每回合自动保存；读档会开出新的时间线，原来那条不会被覆盖。
- **游戏面板**：任务、角色（属性 / 背包 / 难度）、本回合状态、人物关系、时间线，从右下角展开。
- **游戏教程**：第一次打开时由 GM 带你玩一遍示例世界「雾湾」。

### 🌐 联机
- 开房间或加入朋友的房间，每人扮演自己的角色；GM 按顺序处理大家的行动，战斗按先攻轮流。
- 悄悄做的事只有自己和 GM 知道。
- GM 可以用房主电脑上的模型，也可以用云端模型；也能部署到自己的服务器（见 `deploy/`）。

### 🧠 模型
- **内置 llama.cpp**：在「本地模型」里下载默认模型（Rocinante-X-12B，Q4_K_M，约 7 GB）或添加电脑里已有的 GGUF 文件，点「启动」就能用。显卡层数、上下文等参数按你的电脑自动调好。
- **模型市场**：在 Hugging Face / ModelScope 上搜索 GGUF 模型，下载支持暂停、继续和断点续传。
- **云端模型**：支持 OpenAI 兼容接口，内置常用服务商预设（DeepSeek、通义、Kimi、智谱、OpenAI、Claude、OpenRouter 等）。API Key 只保存在 Windows 凭据管理器里，云端服务默认不发送内容，要逐个打开。
- **其他本机服务**：已经在用 Ollama、LM Studio、llama-server 的，可以扫描添加。
- **提示词在线更新**：GM 的提示词可以单独更新，不用重装软件。

### ✍️ 创作
- **剧本**：一句话生成剧本大纲，再逐章展开、完善人物；可以只重写选中的部分。
- **世界树**：地点、人物、势力、物品等设定按树状组织，支持图片、关系图谱和全局搜索（Ctrl+K）。
- **审阅**：AI 的每处修改都先进入「审阅与任务」，你确认后才写入；所有修改都能撤销。

### 🛠️ 软件
- 自研安装 / 卸载程序，当前用户安装，不需要管理员权限。
- 自动更新（安装包有签名校验），可以关闭。
- 托盘与后台运行、开机启动、界面缩放。

## 安装

1. 在 [Releases](https://github.com/Kerxs/ChronicleGM/releases/latest) 下载 `ChronicleGM-<版本>-setup.exe`，双击安装。
2. 需要 WebView2（Windows 11 自带；Windows 10 没有时安装程序会提示安装）。
3. 打开后跟着教程走：先在「设置 → 本地模型」准备模型，再打开示例世界开始玩。

**推荐配置**：内置模型建议 8 GB 以上显存的 NVIDIA 显卡（用 CUDA 运行），也可以用 CPU 运行（较慢）。没有合适的显卡时可以用云端模型。

**数据**：世界、存档、设置和下载的模型都在 `%LOCALAPPDATA%\ChronicleGM`（可以在「设置 → 模型服务 → 关于」里迁移到别的盘）。安装、更新都不会动这些数据；卸载时可以选择是否一起删除。

## 目录

| 路径 | 内容 |
|---|---|
| `crates/rules` | 骰子表达式、剧情条件、效果解析（纯程序判定） |
| `crates/core` | 数据模型、迁移、修订与撤销、变更集、搜索、导入导出、战役状态 |
| `crates/llm` | OpenAI 兼容客户端（重试、流式、取消）、本机服务识别、能力实测 |
| `crates/gen` | 生成管线：上下文组装、提示词模板、输出解析 → 待审变更集 |
| `crates/gm` | AI 主持：回合流程、NPC、导演、战斗 |
| `crates/runtime` | AI 任务调度、模型进程托管、资源监控 |
| `crates/models` | 内置 llama.cpp：硬件检测、运行环境、模型库、下载、自动参数 |
| `crates/protocol` / `crates/server` | 联机协议与服务器 |
| `crates/installer` | 自研安装 / 卸载程序 |
| `src-tauri` | 桌面外壳 |
| `ui` | React 界面 |
| `deploy` | 服务器部署文件 |

## 开发

```bash
cargo test --workspace
npm test
npm run tauri dev
```

打包开发版安装程序（自研安装器）：

```bash
bash scripts/package-dev.sh
```

更多模块说明见 [docs/details.md](docs/details.md)。

## 许可证

[MIT](LICENSE)。第三方组件按各自的许可证使用（例如液态玻璃组件 @samasante/liquid-glass 为 MIT；llama.cpp 为 MIT，运行时按需下载）。
