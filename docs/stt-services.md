# 语音识别服务（语音输入用）

软件只调用 OpenAI 兼容的 `POST /v1/audio/transcriptions`，**不下载、不安装模型**。下面是在 GitHub 上找到、核对过官方说明的本机方案（2026-11-15 查阅）。都已加进「模型服务 → 添加服务 → 语音识别」的预设。

| 项目 | 许可 | 适合 | 启动方式（按官方说明） | 软件里的地址 |
|---|---|---|---|---|
| [whisper.cpp](https://github.com/ggml-org/whisper.cpp)（`whisper-server`） | MIT | 纯本机、可用 CPU / 显卡，Windows 有官方发布包 | `whisper-server --port 8178 --inference-path /v1/audio/transcriptions -l zh -m <ggml 模型.bin>` | `http://127.0.0.1:8178/v1` |
| [FunASR](https://github.com/modelscope/FunASR)（SenseVoice） | 代码 MIT，模型另有许可 | 中文、粤语识别强 | `pip install funasr fastapi uvicorn python-multipart` 后 `funasr-server --host 127.0.0.1 --port 8000 --model sensevoice --device cpu` | `http://127.0.0.1:8000/v1`，模型 `sensevoice` |
| [speaches](https://github.com/speaches-ai/speaches)（原 faster-whisper-server） | MIT | 用 Docker 的话最省事，支持显卡 | 官方推荐 Docker，端口 8000 | `http://127.0.0.1:8000/v1` |

注意：
- whisper-server 默认路径是 `/inference`、默认端口 8080（和你的 llama-server 冲突），所以要加 `--inference-path` 并换端口。它只认 WAV：软件会先把录音转成 16 kHz 单声道 WAV 再发送，所以**不需要** `--convert` 和 ffmpeg。
- FunASR 的部署文档是按 POSIX shell 写的，Windows 上没有官方说明；它的服务没有鉴权，只能监听 127.0.0.1。
- 这些服务都没有在这台电脑上实际运行验证过（需要下载程序和模型，按你的规则由你决定是否安装）。软件这边的请求格式只用本地模拟服务测过。
