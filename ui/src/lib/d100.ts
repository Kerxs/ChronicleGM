// 百分骰（1–100）拆成两颗 d10：十位（0–9，印成 00–90）和个位（0–9）。100 是 00 + 0。
// 单独放一个文件：骰子托盘要用，但不能为此提前加载 3D 引擎。
export function splitD100(v: number): { tens: number; ones: number } {
  const x = ((Math.round(v) % 100) + 100) % 100;
  return { tens: Math.floor(x / 10), ones: x % 10 };
}
