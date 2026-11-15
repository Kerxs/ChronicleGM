// 朗读分段：长讲述按段落和句子切成几段，第一段较短（先合成好就能先开始播放），后面每段一两百字。
// 每段先合成好再播放，文字跟着声音出现；后面的段在前一段播放时合成。

const SENTENCE_END = /([。！？!?；;…]+[”」』"']?)/;

/** 把一段文字切成句子（保留句末标点）。 */
function sentences(p: string): string[] {
  const parts = p.split(SENTENCE_END);
  const out: string[] = [];
  for (let i = 0; i < parts.length; i += 2) {
    const s = (parts[i] ?? "") + (parts[i + 1] ?? "");
    if (s.trim()) out.push(s.trim());
  }
  return out;
}

export function splitForSpeech(text: string, firstMax = 80, max = 200): string[] {
  const paras = text
    .split(/\n+/)
    .map((p) => p.trim())
    .filter(Boolean);
  const out: string[] = [];
  let cur = "";
  const limit = () => (out.length === 0 ? firstMax : max);
  const flush = () => {
    if (cur.trim()) out.push(cur.trim());
    cur = "";
  };
  for (const p of paras) {
    for (const s of sentences(p)) {
      if (cur && cur.length + s.length > limit()) flush();
      cur += s;
      // 一句就超过上限（没有标点的长句）：单独成段。
      if (cur.length >= limit()) flush();
    }
    // 段落之间尽量断开，读起来有停顿；太短的段落和下一段合在一起。
    if (cur.length >= 40) flush();
  }
  flush();
  return out;
}
