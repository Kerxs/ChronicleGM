// 托管配置列表里的一句话摘要。纯函数。
export function cache_ram_label(args: string[]): string {
  const i = args.findIndex((a) => a === "--cache-ram" || a === "-cram");
  if (i < 0) return "提示词缓存：默认 8 GB";
  const v = Number(args[i + 1]);
  if (!Number.isFinite(v)) return "提示词缓存：参数不完整";
  if (v < 0) return "提示词缓存：不限制";
  if (v === 0) return "提示词缓存：关闭";
  return `提示词缓存：最多 ${(v / 1024).toFixed(v % 1024 ? 1 : 0)} GB`;
}
