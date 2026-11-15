// 云端服务预设：只是帮你填好地址与常用模型名，都可以改。
// 地址与请求格式按各家公开文档的 OpenAI 兼容接口填写；没有用真实 API Key 测试过（软件里的"重新测试"会实测）。
import type { ServiceType } from "../bindings/ServiceType";

export interface CloudPreset {
  id: string;
  name: string;
  service: ServiceType;
  base_url: string;
  models: string[];
  /** 语音：可选的声音（第一个作为旁白默认）。 */
  voices?: string[];
  audio_format?: "wav" | "mp3";
  send_instructions?: boolean;
  /** 生图：可选尺寸。 */
  sizes?: string[];
  note?: string;
  /** 本机服务：不需要 Key。 */
  local?: boolean;
  /** 语音接口：不填 = OpenAI 兼容；gpt_sovits = GPT-SoVITS api_v2。 */
  tts_api?: "gpt_sovits";
}

export const CLOUD_PRESETS: CloudPreset[] = [
  // ---------- 语言模型 ----------
  { id: "openai", name: "OpenAI", service: "chat", base_url: "https://api.openai.com/v1", models: ["gpt-4o", "gpt-4o-mini"] },
  { id: "anthropic", name: "Anthropic Claude（OpenAI 兼容接口）", service: "chat", base_url: "https://api.anthropic.com/v1", models: ["claude-sonnet-5-5", "claude-haiku-5-5", "claude-opus-5-5"], note: "使用 Anthropic 提供的 OpenAI 兼容接口；结构化输出等能力以「重新测试」的结果为准。" },
  { id: "gemini", name: "Google Gemini（OpenAI 兼容接口）", service: "chat", base_url: "https://generativelanguage.googleapis.com/v1beta/openai", models: [], note: "2026-11-14 在这台电脑的网络环境下连不上这个地址（可能需要代理）。" },
  { id: "deepseek", name: "DeepSeek 深度求索", service: "chat", base_url: "https://api.deepseek.com/v1", models: ["deepseek-chat", "deepseek-reasoner"] },
  { id: "dashscope", name: "阿里云百炼（通义千问）", service: "chat", base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1", models: ["qwen-plus", "qwen-max", "qwen-turbo"] },
  { id: "moonshot", name: "月之暗面 Kimi", service: "chat", base_url: "https://api.moonshot.cn/v1", models: [] },
  { id: "zhipu", name: "智谱 GLM", service: "chat", base_url: "https://open.bigmodel.cn/api/paas/v4", models: [] },
  { id: "ark", name: "火山方舟（豆包）", service: "chat", base_url: "https://ark.cn-beijing.volces.com/api/v3", models: [], note: "模型名填方舟控制台里的接入点 ID（ep- 开头）或模型 ID。" },
  { id: "siliconflow", name: "硅基流动 SiliconFlow", service: "chat", base_url: "https://api.siliconflow.cn/v1", models: [] },
  { id: "openrouter", name: "OpenRouter", service: "chat", base_url: "https://openrouter.ai/api/v1", models: [] },
  { id: "custom-chat", name: "其他（OpenAI 兼容）", service: "chat", base_url: "", models: [] },
  // ---------- 语音合成 ----------
  {
    id: "openai-tts",
    name: "OpenAI 语音",
    service: "speech",
    base_url: "https://api.openai.com/v1",
    models: ["gpt-4o-mini-tts", "tts-1", "tts-1-hd"],
    voices: ["alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer"],
    audio_format: "mp3",
    send_instructions: true,
    note: "gpt-4o-mini-tts 支持声音描述（语气、情绪）；tts-1 不支持，可以关掉「发送声音描述」。",
  },
  {
    id: "siliconflow-tts",
    name: "硅基流动 语音（CosyVoice）",
    service: "speech",
    base_url: "https://api.siliconflow.cn/v1",
    models: ["FunAudioLLM/CosyVoice2-0.5B"],
    voices: ["alex", "anna", "bella", "benjamin", "charles", "claire", "david", "diana"].map((v) => `FunAudioLLM/CosyVoice2-0.5B:${v}`),
    audio_format: "mp3",
    send_instructions: false,
  },
  {
    id: "kokoro-tts",
    name: "本机 Kokoro-FastAPI",
    service: "speech",
    base_url: "http://127.0.0.1:8880/v1",
    models: ["kokoro"],
    voices: ["zf_xiaobei", "zf_xiaoni", "zf_xiaoxiao", "zf_xiaoyi", "zm_yunjian", "zm_yunxi", "zm_yunxia", "zm_yunyang"],
    audio_format: "mp3",
    send_instructions: false,
    local: true,
    note: "github.com/remsky/Kokoro-FastAPI，默认端口 8880，OpenAI 兼容。z 开头的是中文声音；点「读取声音列表」可以看到服务里实际有哪些。",
  },
  {
    id: "edge-tts",
    name: "本机 openai-edge-tts（微软 Edge 在线语音）",
    service: "speech",
    base_url: "http://127.0.0.1:5050/v1",
    models: ["tts-1"],
    voices: ["zh-CN-XiaoxiaoNeural", "zh-CN-YunxiNeural", "zh-CN-YunjianNeural", "zh-CN-XiaoyiNeural", "zh-CN-YunyangNeural"],
    audio_format: "mp3",
    send_instructions: false,
    local: true,
    note: "github.com/travisvn/openai-edge-tts，默认端口 5050。它在本机运行，但会把文字发给微软的在线语音服务合成。如果它开着 REQUIRE_API_KEY，API Key 填它的 API_KEY（任意字符串）。",
  },
  {
    id: "gpt-sovits",
    name: "本机 GPT-SoVITS（api_v2）",
    service: "speech",
    base_url: "http://127.0.0.1:9880",
    models: ["gpt-sovits"],
    audio_format: "wav",
    send_instructions: false,
    local: true,
    tts_api: "gpt_sovits",
    note: "github.com/RVC-Boss/GPT-SoVITS：在它的目录里运行 python api_v2.py -a 127.0.0.1 -p 9880。需要一段 3–10 秒的参考音频（填 GPT-SoVITS 能读到的路径）和这段音频里说的话。",
  },
  { id: "custom-tts", name: "其他（OpenAI 兼容 /audio/speech，本机或云端）", service: "speech", base_url: "", models: [], audio_format: "mp3", send_instructions: false },
  // ---------- 生成图片 ----------
  { id: "openai-image", name: "OpenAI 图片", service: "image", base_url: "https://api.openai.com/v1", models: ["gpt-image-1", "dall-e-3"], sizes: ["1024x1024", "1024x1536", "1536x1024"] },
  { id: "zhipu-image", name: "智谱 CogView", service: "image", base_url: "https://open.bigmodel.cn/api/paas/v4", models: ["cogview-4"], sizes: ["1024x1024", "768x1344", "1344x768"] },
  { id: "custom-image", name: "其他（OpenAI 兼容 /images/generations）", service: "image", base_url: "", models: [], sizes: ["1024x1024", "1024x1536", "1536x1024", "512x512"] },
  // ---------- 语音识别（语音输入） ----------
  { id: "openai-stt", name: "OpenAI 语音识别", service: "transcription", base_url: "https://api.openai.com/v1", models: ["gpt-4o-mini-transcribe", "whisper-1"] },
  { id: "siliconflow-stt", name: "硅基流动 语音识别", service: "transcription", base_url: "https://api.siliconflow.cn/v1", models: ["FunAudioLLM/SenseVoiceSmall"] },
  {
    id: "whispercpp-stt",
    name: "本机 whisper.cpp（whisper-server）",
    service: "transcription",
    base_url: "http://127.0.0.1:8178/v1",
    models: [],
    local: true,
    note: "github.com/ggml-org/whisper.cpp（MIT）。启动时加 --port 8178 --inference-path /v1/audio/transcriptions -l zh -m <模型.bin>；8080 已被语言模型占用，所以换端口。软件会把录音转成 16 kHz WAV 再发送，不需要 --convert 和 ffmpeg。",
  },
  {
    id: "funasr-stt",
    name: "本机 FunASR SenseVoice（中文识别强）",
    service: "transcription",
    base_url: "http://127.0.0.1:8000/v1",
    models: ["sensevoice"],
    local: true,
    note: "github.com/modelscope/FunASR（代码 MIT，模型另有许可）。按它的 README 安装后运行 funasr-server --host 127.0.0.1 --port 8000 --model sensevoice --device cpu（文档按 POSIX shell 写，Windows 上没有官方说明）。",
  },
  {
    id: "speaches-stt",
    name: "本机 speaches（原 faster-whisper-server）",
    service: "transcription",
    base_url: "http://127.0.0.1:8000/v1",
    models: [],
    local: true,
    note: "github.com/speaches-ai/speaches（MIT），官方推荐用 Docker 运行，默认端口 8000；模型名按它的文档填写。",
  },
  { id: "custom-stt", name: "其他（OpenAI 兼容 /audio/transcriptions）", service: "transcription", base_url: "", models: [] },
];

export const SERVICE_LABEL: Record<ServiceType, string> = { chat: "语言模型", speech: "语音合成", image: "生成图片", transcription: "语音识别" };
