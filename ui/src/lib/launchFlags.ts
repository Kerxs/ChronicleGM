// 托管模型启动参数的可视化编辑：参数目录 + 在参数数组上读写。纯函数。
// 参数名都核对过本机 llama-server（0.8.0-dev build 12063）与 breeze-server（v0.1.6）的 --help。
// 目录里没有的参数原样保留，在「全部参数（高级）」里编辑。

import type { ModelRole } from "../bindings/ModelRole";

export type FlagKind = "int" | "choice" | "switch" | "path" | "text";

export interface FlagSpec {
  key: string;
  /** 参数名，第一个是写入时用的。空数组 + positional 表示第一个位置参数（breeze 的模型文件）。 */
  names: string[];
  positional?: boolean;
  label: string;
  group: string;
  kind: FlagKind;
  /** choice 的选项：[值, 显示]。 */
  choices?: [string, string][];
  /** int 的常用值（下拉建议）。 */
  presets?: [string, string][];
  unit?: string;
  /** 不写这个参数时程序的默认行为。 */
  fallback: string;
  help: string;
  /** 文件选择的扩展名。 */
  ext?: string;
}

const LLAMA: FlagSpec[] = [
  { key: "model", names: ["-m", "--model"], label: "模型文件", group: "模型", kind: "path", ext: "gguf", fallback: "必须填写", help: "主模型 .gguf 文件。" },
  { key: "mmproj", names: ["--mmproj", "-mm"], label: "视觉模块（看图）", group: "模型", kind: "path", ext: "gguf", fallback: "不加载：不能看图", help: "多模态投影文件（mmproj）。图片输入、AI 看图描述、参考图片都需要它；不填就不能看图，但能省下约 1 GB 内存。" },
  { key: "mmproj_cpu", names: ["--no-mmproj-offload"], label: "视觉模块放在内存，不占显存", group: "模型", kind: "switch", fallback: "关：视觉模块放显卡", help: "显存紧张时打开。看图会慢一些。" },
  { key: "ctx", names: ["-c", "--ctx-size"], label: "上下文长度", group: "内存与上下文", kind: "int", unit: "token", presets: [["8192", "8K"], ["16384", "16K"], ["32768", "32K"], ["65536", "64K"]], fallback: "按模型文件（通常很大）", help: "一次能看到的最长内容。越大越占显存/内存。GM 回合一般在 6000–12000 token。" },
  { key: "cache_ram", names: ["--cache-ram", "-cram"], label: "提示词缓存上限", group: "内存与上下文", kind: "int", unit: "MiB", presets: [["1024", "1 GB"], ["2048", "2 GB"], ["4096", "4 GB"], ["8192", "8 GB"], ["0", "关闭"], ["-1", "不限制"]], fallback: "8192 MiB", help: "llama-server 把用过的提示词状态缓存在内存里，下次相同开头时更快。默认最多占 8 GB，内存紧张时是内存持续上涨的主要原因。" },
  { key: "ctk", names: ["-ctk", "--cache-type-k"], label: "KV 缓存精度（K）", group: "内存与上下文", kind: "choice", choices: [["f16", "f16（默认）"], ["q8_0", "q8_0（省一半）"], ["q4_0", "q4_0（更省，可能变差）"]], fallback: "f16", help: "上下文缓存的精度。" },
  { key: "ctv", names: ["-ctv", "--cache-type-v"], label: "KV 缓存精度（V）", group: "内存与上下文", kind: "choice", choices: [["f16", "f16（默认）"], ["q8_0", "q8_0（省一半）"], ["q4_0", "q4_0（更省，可能变差）"]], fallback: "f16", help: "通常与 K 相同。" },
  { key: "ctxcp", names: ["--ctx-checkpoints", "-ctxcp", "--swa-checkpoints"], label: "每个槽的上下文检查点", group: "内存与上下文", kind: "int", presets: [["4", "4"], ["8", "8"], ["32", "32（默认）"]], fallback: "32", help: "混合架构模型（如 Qwen3.5 MoE）回退时用的检查点数量，每个都占内存。" },
  { key: "parallel", names: ["-np", "--parallel"], label: "同时处理的请求数", group: "内存与上下文", kind: "int", presets: [["1", "1"], ["2", "2"], ["4", "4"]], fallback: "自动", help: "槽越多越占内存。本软件的调度中心本来就一次只发一个 GM 请求。" },
  { key: "load", names: ["-lm", "--load-mode"], label: "加载方式", group: "内存与上下文", kind: "choice", choices: [["auto", "自动（mmap）"], ["none", "直接读入"], ["mmap", "mmap"], ["mlock", "锁定在内存（不被换出）"], ["mmap+mlock", "mmap + 锁定"]], fallback: "自动", help: "mlock 让模型不被换到硬盘，但内存不足时其他程序会更卡。" },
  { key: "ngl", names: ["-ngl", "--gpu-layers", "--n-gpu-layers"], label: "放到显卡的层数", group: "显卡", kind: "text", presets: [["auto", "自动"], ["0", "0（全部用 CPU）"], ["99", "全部"]], fallback: "自动", help: "数字或 auto。" },
  { key: "fit", names: ["--fit", "-fit"], label: "按空闲显存自动适配", group: "显卡", kind: "choice", choices: [["on", "开"], ["off", "关"]], fallback: "开", help: "加载时按当时的空闲显存决定放多少到显卡。" },
  { key: "fitt", names: ["--fit-target", "-fitt"], label: "适配时给显卡留的余量", group: "显卡", kind: "int", unit: "MiB", presets: [["1024", "1024（默认）"], ["2048", "2048"], ["3072", "3072"]], fallback: "1024", help: "同一显卡上还有语音模型时要留大一些；也可以用上面的「预留显存」自动加。" },
  { key: "ncmoe", names: ["-ncmoe", "--n-cpu-moe"], label: "放在 CPU 的 MoE 层数", group: "显卡", kind: "int", fallback: "自动", help: "MoE 模型把前 N 层的专家放在内存里，省显存。" },
  { key: "fa", names: ["-fa", "--flash-attn"], label: "Flash Attention", group: "显卡", kind: "choice", choices: [["auto", "自动"], ["on", "开"], ["off", "关"]], fallback: "自动", help: "通常开着更快、更省显存。" },
  { key: "threads", names: ["-t", "--threads"], label: "CPU 线程数", group: "速度", kind: "int", fallback: "自动", help: "一般设为物理核心数。" },
  { key: "batch", names: ["-b", "--batch-size"], label: "批大小", group: "速度", kind: "int", presets: [["512", "512"], ["2048", "2048（默认）"]], fallback: "2048", help: "" },
  { key: "ubatch", names: ["-ub", "--ubatch-size"], label: "物理批大小", group: "速度", kind: "int", presets: [["512", "512（默认）"], ["1024", "1024"], ["2048", "2048"]], fallback: "512", help: "越大读提示词越快，但计算缓冲更大。" },
  { key: "reasoning", names: ["-rea", "--reasoning"], label: "思考模式", group: "思考", kind: "choice", choices: [["auto", "按模板"], ["on", "开"], ["off", "关"]], fallback: "按模板", help: "本软件会按需单独关闭思考（设置页「思考模式」），一般不用在这里改。" },
  { key: "budget", names: ["--reasoning-budget"], label: "思考长度上限", group: "思考", kind: "int", unit: "token", presets: [["-1", "不限"], ["2048", "2048"], ["4096", "4096"]], fallback: "不限", help: "限制思考长度能明显加快复杂任务。" },
  { key: "host", names: ["--host"], label: "监听地址", group: "网络", kind: "text", fallback: "127.0.0.1", help: "保持 127.0.0.1（只允许本机访问）。" },
  { key: "port", names: ["--port"], label: "端口", group: "网络", kind: "int", fallback: "8080", help: "要与上面的「端口」一致。" },
];

const BREEZE: FlagSpec[] = [
  { key: "model", names: [], positional: true, label: "模型文件", group: "模型", kind: "path", ext: "gguf", fallback: "必须填写", help: "breeze-server 的第一个参数是 .gguf 模型文件。" },
  { key: "voices", names: ["--voices-dir"], label: "音色文件夹", group: "模型", kind: "path", fallback: "voices", help: "" },
  { key: "cpu", names: ["--cpu"], label: "只用 CPU（不占显存）", group: "设备", kind: "switch", fallback: "关：用显卡", help: "实测 CPU 模式实时率约 0.39，比显卡慢，但不占显存。" },
  { key: "parallel", names: ["--parallel"], label: "同时合成的数量", group: "设备", kind: "int", fallback: "1", help: "" },
  { key: "chunk_first", names: ["--chunk-first"], label: "首段帧数", group: "速度", kind: "int", fallback: "1", help: "越小越早出声。" },
  { key: "chunk_max", names: ["--chunk-max"], label: "最大分段帧数", group: "速度", kind: "int", fallback: "25", help: "越大越省算力。" },
  { key: "split", names: ["--split-chars"], label: "长文本切分长度", group: "速度", kind: "int", unit: "字", fallback: "600", help: "0 = 不切分。" },
  { key: "nowarm", names: ["--no-warmup"], label: "跳过启动预热", group: "速度", kind: "switch", fallback: "关", help: "启动快一点，但第一句话慢。" },
  { key: "host", names: ["--host"], label: "监听地址", group: "网络", kind: "text", fallback: "127.0.0.1", help: "" },
  { key: "port", names: ["--port"], label: "端口", group: "网络", kind: "int", fallback: "8080", help: "要与上面的「端口」一致。" },
];

/** 按程序名与类型选目录。认不出的程序返回空数组（只显示原始参数）。 */
export function catalogFor(exe: string, role: ModelRole): FlagSpec[] {
  const name = exe.split(/[\\/]/).pop()?.toLowerCase() ?? "";
  if (name.startsWith("breeze")) return BREEZE;
  if (name.startsWith("llama-server") || (role === "llm" && !name)) return LLAMA;
  return [];
}

const isFlag = (s: string) => /^-{1,2}[a-zA-Z]/.test(s);

function indexOf(args: string[], spec: FlagSpec): number {
  if (spec.positional) return args.length && !isFlag(args[0]) ? 0 : -1;
  return args.findIndex((a) => spec.names.includes(a));
}

/** 读取参数值：switch 返回 "on" 或 null；其余返回值或 null（没写）。 */
export function getFlag(args: string[], spec: FlagSpec): string | null {
  const i = indexOf(args, spec);
  if (i < 0) return null;
  if (spec.kind === "switch") return "on";
  if (spec.positional) return args[0];
  const v = args[i + 1];
  return v !== undefined && !(isFlag(v) && !/^-\d/.test(v)) ? v : "";
}

/** 写入参数：value 为 null 时删除这个参数；返回新数组，不改原数组。 */
export function setFlag(args: string[], spec: FlagSpec, value: string | null): string[] {
  const a = [...args];
  const i = indexOf(a, spec);
  if (spec.positional) {
    if (value == null) return i === 0 ? a.slice(1) : a;
    if (i === 0) a[0] = value;
    else a.unshift(value);
    return a;
  }
  const hasValue = spec.kind !== "switch";
  if (i >= 0) {
    const old = getFlag(a, spec);
    const width = hasValue && old !== "" ? 2 : 1;
    if (value == null) a.splice(i, width);
    else if (hasValue) a.splice(i, width, a[i], value);
    return a;
  }
  if (value == null) return a;
  return hasValue ? [...a, spec.names[0], value] : [...a, spec.names[0]];
}

/** 目录覆盖不到的参数（按原顺序），用来提示"还有这些参数"。 */
export function unknownArgs(args: string[], catalog: FlagSpec[]): string[] {
  const used = new Set<number>();
  for (const s of catalog) {
    const i = indexOf(args, s);
    if (i < 0) continue;
    used.add(i);
    if (s.kind !== "switch" && !s.positional && getFlag(args, s) !== "") used.add(i + 1);
  }
  return args.filter((_, i) => !used.has(i));
}

export function groups(catalog: FlagSpec[]): [string, FlagSpec[]][] {
  const m = new Map<string, FlagSpec[]>();
  for (const s of catalog) m.set(s.group, [...(m.get(s.group) ?? []), s]);
  return [...m.entries()];
}
