// 语音输入：按一下开始录音，再按一下结束，识别出的文字填进输入框（需要配置语音识别服务）。
import { useEffect, useRef, useState } from "react";
import { api } from "../api";

/** 把浏览器录到的音频（webm/opus 等）转成 16 kHz 单声道 16 位 WAV。转换失败时返回 null（按原格式发送）。 */
async function toWav16k(blob: Blob): Promise<Blob | null> {
  try {
    const ctx = new AudioContext();
    const decoded = await ctx.decodeAudioData(await blob.arrayBuffer());
    void ctx.close();
    const rate = 16000;
    const off = new OfflineAudioContext(1, Math.max(1, Math.ceil(decoded.duration * rate)), rate);
    const src = off.createBufferSource();
    src.buffer = decoded;
    src.connect(off.destination);
    src.start();
    const pcm = (await off.startRendering()).getChannelData(0);
    const buf = new ArrayBuffer(44 + pcm.length * 2);
    const v = new DataView(buf);
    const str = (o: number, s: string) => [...s].forEach((c, i) => v.setUint8(o + i, c.charCodeAt(0)));
    str(0, "RIFF");
    v.setUint32(4, 36 + pcm.length * 2, true);
    str(8, "WAVE");
    str(12, "fmt ");
    v.setUint32(16, 16, true);
    v.setUint16(20, 1, true);
    v.setUint16(22, 1, true);
    v.setUint32(24, rate, true);
    v.setUint32(28, rate * 2, true);
    v.setUint16(32, 2, true);
    v.setUint16(34, 16, true);
    str(36, "data");
    v.setUint32(40, pcm.length * 2, true);
    for (let i = 0; i < pcm.length; i++) v.setInt16(44 + i * 2, Math.max(-1, Math.min(1, pcm[i])) * 0x7fff, true);
    return new Blob([buf], { type: "audio/wav" });
  } catch {
    return null;
  }
}

export default function MicButton({ disabled, onText, notify }: { disabled: boolean; onText: (t: string) => void; notify: (m: string, k?: "info" | "error") => void }) {
  const [available, setAvailable] = useState(false);
  const [state, setState] = useState<"idle" | "rec" | "busy">("idle");
  const rec = useRef<MediaRecorder | null>(null);
  const chunks = useRef<Blob[]>([]);
  useEffect(() => {
    api.transcriptionAvailable().then(setAvailable).catch(() => setAvailable(false));
  }, []);
  if (!available) return null;

  async function start() {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const r = new MediaRecorder(stream);
      chunks.current = [];
      r.ondataavailable = (e) => e.data.size && chunks.current.push(e.data);
      r.onstop = async () => {
        stream.getTracks().forEach((t) => t.stop());
        const raw = new Blob(chunks.current, { type: r.mimeType || "audio/webm" });
        setState("busy");
        try {
          const blob = (await toWav16k(raw)) ?? raw;
          const buf = new Uint8Array(await blob.arrayBuffer());
          let bin = "";
          for (let i = 0; i < buf.length; i += 0x8000) bin += String.fromCharCode(...buf.subarray(i, i + 0x8000));
          const text = await api.transcribeAudio(btoa(bin), blob.type);
          if (text.trim()) onText(text.trim());
          else notify("没有识别出文字", "error");
        } catch (e) {
          notify((e as Error).message, "error");
        } finally {
          setState("idle");
        }
      };
      rec.current = r;
      r.start();
      setState("rec");
    } catch (e) {
      notify("无法使用麦克风：" + (e as Error).message, "error");
    }
  }

  return (
    <button
      type="button"
      className={`mic${state === "rec" ? " recording" : ""}`}
      disabled={disabled || state === "busy"}
      title={state === "rec" ? "再按一下结束录音" : "语音输入"}
      onClick={() => (state === "rec" ? rec.current?.stop() : void start())}
    >
      {state === "busy" ? "识别中…" : state === "rec" ? "■ 结束" : "🎤"}
    </button>
  );
}
