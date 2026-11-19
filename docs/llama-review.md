# 内置 llama.cpp 与模型管理系统：现状审查与实施方案

## 一、现有结构（已核对代码）

| 位置 | 现在做什么 | 和新任务的关系 |
|---|---|---|
| `crates/runtime/src/procs.rs` `ModelManager` | **托管模式**：用户自己填可执行文件路径和启动参数，软件负责启动、停止、重启、崩溃自动重启、按需加载、空闲释放、互斥组、显存预留（`--fit-target`）；只停自己启动的进程 | 推理服务管理器直接**复用**它：新功能自动生成一份 `ModelProfile`（exe 指向内置的 llama-server，参数自动算），不再另写一套进程管理 |
| `crates/runtime/src/estimate.rs` `estimate_llama` | 按启动参数估算显存和内存 | 用于自动参数配置和启动前的兼容性检查 |
| `crates/runtime/src/monitor.rs` | 资源监控：nvidia-smi 读显存、进程 CPU 和内存、按模型统计内存 | 用于运行状态面板（显存、内存、CPU） |
| `crates/runtime/src/sched.rs` | AI 任务调度中心（所有模型请求都经过它） | 不变，游戏仍经调度中心调用 |
| `crates/llm/src/gguf.rs` | 读 GGUF 头（架构、上下文、量化、聊天模板）、扫描文件夹 | 用于模型库的模型信息和 **GGUF 结构检查** |
| `crates/llm/src/discovery.rs` | 识别本机已运行的服务、能力检测、模型列表 | 内置服务启动后自动登记为一个"模型服务" |
| `crates/llm/src/secrets.rs` + `src-tauri/src/secrets.rs` | API Key 存在系统钥匙串（Windows 凭据管理器） | Hugging Face 和魔搭的访问令牌也存这里，不写日志 |
| `src-tauri/src/perf.rs` | 托管配置的读写、`sync_profiles` | 内置模型的托管配置从这里注入 |
| `src-tauri/src/update.rs` | 程序自身的自动更新（GitHub Releases，带签名） | 参考它的流程；llama.cpp 更新另写，因为官方构建没有 minisign 签名 |
| 依赖 | `ureq`、`sha2`、`zip`、`flate2`、`tar`、`sysinfo` 都已经在 Cargo.lock 里 | 不需要引入新的大依赖 |

**与原设计原则冲突的地方**：项目原本定的原则是"模型用户自备，软件只做识别，不下载模型、不替用户选择模型"（procs.rs 文件头写明）。这次任务书要求内置下载和管理，是对这条原则的有意改变。按任务书执行，但保留"用户自备"路线：已有的托管配置和外部服务照常可用，也不会被自动接管。

## 二、新增模块（相互独立）

```
crates/models/                新 crate（不依赖 Tauri，可单独测试）
├─ runtime_mgr.rs   llama.cpp 运行环境：查询 Releases、选构建、下载、解压、校验、版本目录、切换、回滚、损坏检测
├─ hw.rs            硬件检测：系统、CPU 架构与指令集、NVIDIA（nvidia-smi 版本、驱动、CUDA）、Vulkan
├─ repo/mod.rs      RepoAdapter trait：search / details / files
├─ repo/hf.rs       Hugging Face 公开 API（/api/models、/api/models/{id}/tree、resolve 下载、Bearer 令牌）
├─ repo/modelscope.rs 魔搭公开 API（模型搜索、文件列表、下载地址、令牌）
├─ download.rs      下载管理器：任务持久化（JSON）、Range 续传、暂停 / 继续 / 取消、重试退避、并发上限、
│                   速度与剩余时间、大小与 SHA256 校验、GGUF 结构检查、.part 临时文件、完成后才原子移入模型库、按 SHA / 路径去重
├─ library.rs       模型库：记录、添加已有文件、删除记录 / 删除文件（界面二次确认）、更改保存位置（移动文件）、
│                   mmproj 关联、默认模型、"正在使用"
└─ tune.rs          自动参数：GPU 层数、上下文、线程、批大小、KV 缓存类型、Flash Attention、显存与内存预算；启动前兼容性检查
src-tauri/src/models.rs        Tauri 命令（耗时操作都在后台线程），进度事件推给界面
ui/src/pages/Models*.tsx       四个页面：模型库 / 模型市场 / 下载管理 / 运行环境
```

- 推理服务：用 `tune.rs` 生成参数，生成 `ModelProfile` 交给已有的 `ModelManager` 启停，只监听 127.0.0.1。
  - 启动后登记成一个"模型服务"，游戏照常经调度中心调用。
  - 运行状态：显存、内存、CPU 来自已有的 monitor；tokens/s 和上下文用量来自 llama-server 的 `/slots`、`/metrics`。
- 数据位置：都在 `数据目录/llama/`。
  - 运行环境：`runtimes/<版本>-<后端>/`，`current.json` 记录当前版本；
  - 下载任务：`downloads.json` 和 `*.part` 临时文件；
  - 模型默认放 `数据目录/models/`，可以改。

  更新和重装只动 `runtimes/`，不碰模型、配置和存档。
- 失败恢复：新版本先解压到临时目录，用 `llama-server --version` 和一次 `/health` 自检，通过后才切换 `current.json`；失败时自动退回旧版本。最近两个版本都保留。
- 安全：
  - 只从 `github.com/ggml-org/llama.cpp` 的 Releases、`huggingface.co`、`modelscope.cn` 下载；
  - 仓库里的模型卡、说明文字只当文本显示，不执行；
  - 下载的文件只在 GGUF 头检查通过后才进入模型库；
  - 令牌存钥匙串，日志里只写"已设置令牌"。

## 三、阶段计划

| 阶段 | 内容 | 验证 |
|---|---|---|
| 1 | Releases 查询、按硬件选构建、下载、解压、版本目录、切换、回滚、损坏检测与重装 | 单元测试（模拟 Releases 和压缩包）；真实下载一次 Windows 构建并启动 `--version` |
| 2 | Hugging Face 与魔搭适配器 | 模拟服务器测试；真实搜索各一次 |
| 3 | 下载管理器 | 模拟服务器：断点续传、暂停继续、重启恢复、校验失败、去重、并发上限 |
| 4 | 模型库与完整性检查 | 单元测试；添加 D:\llama 已有 GGUF |
| 5 | llama-server 启停与自动参数 | 参数推算单测；真实启动一个小模型，读 tokens/s |
| 6 | 游戏接入与端到端 | 用内置服务跑一局真实试玩；全量单人和联机回归 |

## 四、用户决定（2026-11-18）

1. **以内置为主**：内置下载和管理是默认路线；原来的托管模式（手填程序路径和参数）和外部服务降为高级选项，但继续保留。
2. 6 个阶段**全部自动做完再统一确认**。
3. **不做真实下载**：llama.cpp 构建和模型都用模拟服务器测试；真实下载由用户在应用里自己试。
4. **默认模型**：Hugging Face `mradermacher/Rocinante-X-12B-v1-Heretic-Uncensored-GGUF` 里的 `Rocinante-X-12B-v1-Heretic-Uncensored.Q4_K_M.gguf`。模型市场把它列为推荐的默认模型，首次使用时一键下载。
