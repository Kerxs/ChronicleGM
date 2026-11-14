// 用生图服务给节点生成图片：描述按节点设定预填（不含主持人隐藏内容），可以改；生成后预览，确认才保存到节点。
import { useState } from "react";
import { api } from "../api";
import type { Ctx } from "../App";
import type { Node } from "../bindings/Node";
import { promptFor } from "../lib/imagePrompt";
import { makeThumb } from "../lib/thumbs";

const STYLES: [string, string][] = [
  ["", "不指定风格"],
  ["奇幻插画风格，细腻光影", "奇幻插画"],
  ["水彩画风格，柔和色调", "水彩"],
  ["厚涂油画风格", "油画"],
  ["日式动画风格", "动画"],
  ["写实摄影风格", "写实"],
  ["古地图风格，羊皮纸质感，标注地名", "古地图"],
  ["像素画风格", "像素"],
];

export default function ImageGenPanel({ ctx, node, onSaved, onClose }: { ctx: Ctx; node: Node; onSaved: () => void; onClose: () => void }) {
  const [style, setStyle] = useState(STYLES[1][0]);
  const [prompt, setPrompt] = useState(() => promptFor(node, STYLES[1][0]));
  const [size, setSize] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ data_url: string; revised_prompt: string | null; provider: string; ms: number } | null>(null);
  const [saving, setSaving] = useState(false);

  async function gen() {
    setBusy(true);
    setResult(null);
    try {
      setResult(await api.generateImage(prompt, size || null));
    } catch (e) {
      ctx.notify(`生成失败：${(e as Error).message}`, "error");
    } finally {
      setBusy(false);
    }
  }

  async function save() {
    if (!result) return;
    setSaving(true);
    try {
      const img = new Image();
      await new Promise((r, j) => {
        img.onload = r;
        img.onerror = j;
        img.src = result.data_url;
      });
      const saved = await api.nodeImageAdd(ctx.worldId, node.id, result.data_url, img.naturalWidth, img.naturalHeight, `AI 生成：${prompt.split("\n")[0].slice(0, 60)}`);
      try {
        await api.nodeImageSetThumb(ctx.worldId, saved.id, await makeThumb(result.data_url));
      } catch {
        /* 缩略图之后显示时再补 */
      }
      ctx.notify("已保存到节点图片");
      setResult(null);
      onSaved();
    } catch (e) {
      ctx.notify((e as Error).message, "error");
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="imggen">
      <div className="row">
        <h4 style={{ margin: 0 }}>AI 生成图片</h4>
        <span className="spacer" />
        <button className="small link" onClick={onClose}>
          收起
        </button>
      </div>
      <p className="hint">描述按本节点的设定预填（不包含主持人隐藏内容），可以修改。会发给你在「模型服务」里设置的生图服务，云端服务按次计费。生成后先预览，确认才保存。</p>
      <div className="row wrap">
        <label className="inline">
          风格
          <select
            value={style}
            onChange={(e) => {
              setStyle(e.target.value);
              setPrompt(promptFor(node, e.target.value));
            }}
          >
            {STYLES.map(([v, t]) => (
              <option key={t} value={v}>
                {t}
              </option>
            ))}
          </select>
        </label>
        <label className="inline">
          尺寸
          <select value={size} onChange={(e) => setSize(e.target.value)}>
            <option value="">服务默认</option>
            <option value="1024x1024">1024×1024 方形</option>
            <option value="1024x1536">1024×1536 竖版</option>
            <option value="1536x1024">1536×1024 横版</option>
          </select>
        </label>
      </div>
      <textarea rows={5} value={prompt} onChange={(e) => setPrompt(e.target.value)} />
      <div className="row">
        <button className="primary" disabled={busy || !prompt.trim()} onClick={() => void gen()}>
          {busy ? "正在生成…（可能需要几十秒）" : result ? "重新生成" : "生成"}
        </button>
      </div>
      {busy && <div className="imggen-wait"><span className="tb-spinner" /> 等待生图服务返回…</div>}
      {result && (
        <div className="imggen-result">
          <img src={result.data_url} alt="生成的图片" />
          <div className="hint">
            {result.provider} · {(result.ms / 1000).toFixed(1)} 秒{result.revised_prompt ? ` · 服务改写后的描述：${result.revised_prompt}` : ""}
          </div>
          <div className="row">
            <button className="primary" disabled={saving} onClick={() => void save()}>
              {saving ? "保存中…" : "保存到节点"}
            </button>
            <button onClick={() => setResult(null)}>丢弃</button>
          </div>
        </div>
      )}
    </div>
  );
}
