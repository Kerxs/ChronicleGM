// 把粘贴进来的命令行（可以是 .bat 里用 ^ 续行的多行）拆成程序路径与参数。纯函数。
// 只做拆分，不执行任何东西；%VAR% 这类变量原样保留并提示用户替换。

export interface ParsedCommand {
  exe: string;
  args: string[];
  /** 参数里出现了 %VAR% 之类的环境变量，需要用户自己替换。 */
  unresolved: string[];
  port: number | null;
}

export function parseCommandLine(text: string): ParsedCommand {
  // 去掉 bat 的续行符（行尾 ^），把多行合并成一行；忽略 rem 注释与 @echo 等行。
  const all = text.split(/\r?\n/).map((l) => l.trim());
  // bat 里的 set 变量（set NAME=值 或 set "NAME=值"），用来替换后面的 %NAME%；值为空时这个变量就是空的。
  const vars = new Map<string, string>();
  for (const l of all) {
    const m = /^set\s+"?([A-Za-z_][\w]*)=(.*?)"?$/i.exec(l);
    if (m) vars.set(m[1].toUpperCase(), m[2].trim());
  }
  const lines = all.filter((l) => l && !/^(rem\s|::|@echo|echo\s|chcp|cd\s|set\s|pause$)/i.test(l));
  const joined = lines
    .map((l) => l.replace(/\^$/, "").trim())
    .join(" ")
    .replace(/%([A-Za-z_]\w*)%/g, (m, k: string) => vars.get(k.toUpperCase()) ?? m);
  const tokens: string[] = [];
  let cur = "";
  let quoted = false;
  let has = false;
  for (const ch of joined) {
    if (ch === '"') {
      quoted = !quoted;
      has = true;
    } else if (/\s/.test(ch) && !quoted) {
      if (has || cur) tokens.push(cur);
      cur = "";
      has = false;
    } else {
      cur += ch;
    }
  }
  if (has || cur) tokens.push(cur);
  const [exe = "", ...rest] = tokens;
  const unresolved = rest.filter((a) => /%[^%]+%/.test(a));
  const args = rest.filter((a) => !/^%[^%]+%$/.test(a));
  const pi = args.findIndex((a) => a === "--port");
  const port = pi >= 0 ? Number(args[pi + 1]) || null : null;
  return { exe, args, unresolved, port };
}
