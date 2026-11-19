// 声音：程序用 WebAudio 合成音效、环境音和背景乐（不带第三方素材）。
// 数据目录 audio/ 里放同名文件（例如 unlock.mp3、bgm_combat.ogg）就会替换合成的声音。
// 音量和演出强度存在本机（localStorage），读写失败时用默认值。
import { api } from "../api";

export type FxLevel = "full" | "simple" | "off";

function readNum(k: string, d: number) {
  try {
    const v = localStorage.getItem(k);
    return v === null ? d : Math.min(1, Math.max(0, Number(v)));
  } catch {
    return d;
  }
}
function write(k: string, v: string) {
  try {
    localStorage.setItem(k, v);
  } catch {
    /* 存不了就算了 */
  }
}

export const fxSettings = {
  get level(): FxLevel {
    try {
      const v = localStorage.getItem("fx.level");
      return v === "simple" || v === "off" ? v : "full";
    } catch {
      return "full";
    }
  },
  set level(v: FxLevel) {
    write("fx.level", v);
    if (v === "off") stopMusic();
  },
  get sfx() {
    return readNum("fx.sfx", 0.6);
  },
  set sfx(v: number) {
    write("fx.sfx", String(v));
  },
  get bgm() {
    return readNum("fx.bgm", 0.25);
  },
  set bgm(v: number) {
    write("fx.bgm", String(v));
    if (musicGain) musicGain.gain.value = v * 0.5;
  },
};

let ctx: AudioContext | null = null;
function ac(): AudioContext | null {
  if (typeof window === "undefined" || !("AudioContext" in window)) return null;
  if (!ctx) ctx = new AudioContext();
  if (ctx.state === "suspended") void ctx.resume().catch(() => {});
  return ctx;
}

// ---------- 用户替换的文件 ----------
let overrides: Set<string> | null = null;
const loaded = new Map<string, string>();
async function override(name: string): Promise<string | null> {
  if (!overrides) {
    overrides = new Set();
    try {
      for (const f of await api.audioList()) overrides.add(f);
    } catch {
      /* 浏览器里调试时没有后端 */
    }
  }
  const file = [...overrides].find((f) => f.replace(/\.[^.]+$/, "") === name);
  if (!file) return null;
  if (!loaded.has(file)) {
    try {
      loaded.set(file, await api.audioGet(file));
    } catch {
      return null;
    }
  }
  return loaded.get(file) ?? null;
}
/** 用户往文件夹里加了文件后重新读取。 */
export function reloadOverrides() {
  overrides = null;
  loaded.clear();
}

// ---------- 合成的积木 ----------
function tone(a: AudioContext, out: AudioNode, freq: number, t0: number, dur: number, type: OscillatorType = "sine", vol = 0.3, slideTo?: number) {
  const o = a.createOscillator();
  const g = a.createGain();
  o.type = type;
  o.frequency.setValueAtTime(freq, t0);
  if (slideTo) o.frequency.exponentialRampToValueAtTime(slideTo, t0 + dur);
  g.gain.setValueAtTime(0.0001, t0);
  g.gain.exponentialRampToValueAtTime(vol, t0 + Math.min(0.02, dur / 4));
  g.gain.exponentialRampToValueAtTime(0.0001, t0 + dur);
  o.connect(g).connect(out);
  o.start(t0);
  o.stop(t0 + dur + 0.05);
}

let noiseBuf: AudioBuffer | null = null;
function noise(a: AudioContext, out: AudioNode, t0: number, dur: number, vol = 0.2, filter = 2000, q = 0.7, type: BiquadFilterType = "bandpass") {
  if (!noiseBuf) {
    noiseBuf = a.createBuffer(1, a.sampleRate * 2, a.sampleRate);
    const d = noiseBuf.getChannelData(0);
    for (let i = 0; i < d.length; i++) d[i] = Math.random() * 2 - 1;
  }
  const s = a.createBufferSource();
  s.buffer = noiseBuf;
  const f = a.createBiquadFilter();
  f.type = type;
  f.frequency.value = filter;
  f.Q.value = q;
  const g = a.createGain();
  g.gain.setValueAtTime(0.0001, t0);
  g.gain.exponentialRampToValueAtTime(vol, t0 + 0.01);
  g.gain.exponentialRampToValueAtTime(0.0001, t0 + dur);
  s.connect(f).connect(g).connect(out);
  s.start(t0, Math.random());
  s.stop(t0 + dur + 0.05);
}

const N = (semi: number, base = 220) => base * Math.pow(2, semi / 12);

/** 音效名：dice click unlock fail success discover item clue npc place battle heartbeat turning prologue ending alarm */
export async function play(name: string) {
  const lvl = fxSettings.level;
  if (lvl === "off") return;
  const vol = fxSettings.sfx;
  if (vol <= 0) return;
  const file = await override(name);
  if (file) {
    const el = new Audio(file);
    el.volume = vol;
    void el.play().catch(() => {});
    return;
  }
  const a = ac();
  if (!a) return;
  const out = a.createGain();
  out.gain.value = vol;
  out.connect(a.destination);
  const t = a.currentTime + 0.01;
  switch (name) {
    case "click":
      tone(a, out, 1800, t, 0.04, "square", 0.08);
      break;
    case "dice":
      for (let i = 0; i < 6; i++) noise(a, out, t + i * 0.07 + Math.random() * 0.03, 0.05, 0.25 - i * 0.03, 3000 + Math.random() * 2000, 2);
      break;
    case "unlock":
      noise(a, out, t, 0.05, 0.3, 4000, 4);
      noise(a, out, t + 0.12, 0.06, 0.35, 3000, 4);
      tone(a, out, 600, t + 0.2, 0.25, "triangle", 0.15, 900);
      break;
    case "fail":
      tone(a, out, 160, t, 0.35, "sawtooth", 0.12, 90);
      noise(a, out, t, 0.2, 0.15, 400, 1, "lowpass");
      break;
    case "success":
      tone(a, out, N(7, 440), t, 0.18, "triangle", 0.18);
      tone(a, out, N(12, 440), t + 0.1, 0.35, "triangle", 0.18);
      break;
    case "discover":
      [0, 4, 7, 12].forEach((s, i) => tone(a, out, N(s, 523), t + i * 0.07, 0.4, "sine", 0.14));
      break;
    case "item":
      [12, 16, 19, 24].forEach((s, i) => tone(a, out, N(s, 440), t + i * 0.05, 0.5, "sine", 0.1));
      noise(a, out, t, 0.6, 0.05, 8000, 1, "highpass");
      break;
    case "clue":
      tone(a, out, N(0, 660), t, 1.2, "sine", 0.18);
      tone(a, out, N(7, 660), t + 0.02, 1.2, "sine", 0.08);
      break;
    case "npc":
      tone(a, out, N(-12, 220), t, 1.2, "sine", 0.12, N(-5, 220));
      tone(a, out, N(-5, 220), t + 0.1, 1.1, "triangle", 0.06);
      break;
    case "place":
      noise(a, out, t, 1.6, 0.12, 600, 0.5, "lowpass");
      [0, 7, 12, 16].forEach((s) => tone(a, out, N(s, 196), t + 0.2, 1.8, "sine", 0.05));
      break;
    case "battle":
      for (let i = 0; i < 4; i++) {
        tone(a, out, 90, t + i * 0.22, 0.25, "sine", 0.5, 45);
        noise(a, out, t + i * 0.22, 0.12, 0.2, 200, 1, "lowpass");
      }
      tone(a, out, N(0, 110), t + 0.9, 0.8, "sawtooth", 0.08);
      break;
    case "heartbeat":
      for (let i = 0; i < 3; i++) {
        tone(a, out, 60, t + i * 0.8, 0.15, "sine", 0.6, 40);
        tone(a, out, 55, t + i * 0.8 + 0.22, 0.18, "sine", 0.45, 38);
      }
      break;
    case "turning":
      tone(a, out, 55, t, 2.4, "sine", 0.5, 40);
      noise(a, out, t, 2.2, 0.1, 300, 0.4, "lowpass");
      [0, 3, 7].forEach((s) => tone(a, out, N(s, 147), t + 0.4, 2.2, "triangle", 0.05));
      break;
    case "prologue":
      [0, 4, 7, 11, 14].forEach((s, i) => tone(a, out, N(s, 174), t + i * 0.18, 3, "sine", 0.07));
      noise(a, out, t, 2.5, 0.04, 5000, 0.3, "highpass");
      break;
    case "ending":
      [14, 11, 7, 4, 0].forEach((s, i) => tone(a, out, N(s, 196), t + i * 0.3, 3.2, "sine", 0.07));
      break;
    case "alarm":
      for (let i = 0; i < 4; i++) tone(a, out, i % 2 ? 660 : 880, t + i * 0.25, 0.22, "square", 0.08);
      break;
    default:
      tone(a, out, 880, t, 0.1, "sine", 0.1);
  }
}

// ---------- 背景乐与环境音（生成式：几层慢慢起伏的和弦） ----------
let music: { stop: () => void; mood: string } | null = null;
let musicGain: GainNode | null = null;
const CHORDS: Record<string, number[][]> = {
  opening: [[0, 4, 7, 11], [5, 9, 12, 16], [-3, 0, 4, 7], [2, 5, 9, 12]],
  explore: [[0, 7, 12, 16], [-5, 2, 7, 11], [-3, 4, 7, 12], [-7, 0, 5, 9]],
  combat: [[0, 3, 7, 10], [-2, 2, 5, 9], [-4, 0, 3, 7], [-5, -1, 2, 7]],
  ending: [[0, 4, 7, 14], [-3, 0, 4, 9], [-7, -3, 0, 5], [-5, -1, 2, 7]],
};

export async function startMusic(mood: "opening" | "explore" | "combat" | "ending") {
  if (fxSettings.level !== "full" || fxSettings.bgm <= 0) return stopMusic();
  if (music?.mood === mood) return;
  stopMusic();
  const file = await override(`bgm_${mood}`);
  if (file) {
    const el = new Audio(file);
    el.loop = true;
    el.volume = fxSettings.bgm;
    void el.play().catch(() => {});
    music = { mood, stop: () => el.pause() };
    return;
  }
  const a = ac();
  if (!a) return;
  musicGain = a.createGain();
  musicGain.gain.value = 0;
  musicGain.gain.linearRampToValueAtTime(fxSettings.bgm * 0.5, a.currentTime + 3);
  const lp = a.createBiquadFilter();
  lp.type = "lowpass";
  lp.frequency.value = mood === "combat" ? 1400 : 900;
  musicGain.connect(lp).connect(a.destination);
  const chords = CHORDS[mood];
  const base = mood === "combat" ? 110 : 130.8;
  const len = mood === "combat" ? 2.4 : 5;
  let step = 0;
  let alive = true;
  const g = musicGain;
  const tick = () => {
    if (!alive || !ctx) return;
    const t = ctx.currentTime + 0.05;
    for (const s of chords[step % chords.length]) {
      tone(ctx, g, N(s, base), t, len * 1.1, "sine", 0.06);
      tone(ctx, g, N(s, base) * 1.003, t, len * 1.1, "triangle", 0.025);
    }
    if (mood === "combat") for (let i = 0; i < 4; i++) tone(ctx, g, 70, t + i * (len / 4), 0.2, "sine", 0.25, 40);
    step++;
  };
  tick();
  const h = window.setInterval(tick, len * 1000);
  music = {
    mood,
    stop: () => {
      alive = false;
      clearInterval(h);
      try {
        g.gain.cancelScheduledValues(a.currentTime);
        g.gain.setValueAtTime(g.gain.value, a.currentTime);
        g.gain.linearRampToValueAtTime(0, a.currentTime + 1.5);
      } catch {
        /* 已经停了 */
      }
    },
  };
}

export function stopMusic() {
  music?.stop();
  music = null;
}

let ambient: { stop: () => void; kind: string } | null = null;
/** 环境音：wind / rain / town / cave / sea；空字符串停止。 */
export async function startAmbient(kind: string) {
  if (fxSettings.level !== "full" || !kind) {
    ambient?.stop();
    ambient = null;
    return;
  }
  if (ambient?.kind === kind) return;
  ambient?.stop();
  const file = await override(`amb_${kind}`);
  if (file) {
    const el = new Audio(file);
    el.loop = true;
    el.volume = fxSettings.bgm * 0.6;
    void el.play().catch(() => {});
    ambient = { kind, stop: () => el.pause() };
    return;
  }
  const a = ac();
  if (!a) return;
  if (!noiseBuf) noise(a, a.createGain(), a.currentTime, 0.01, 0.0001);
  const src = a.createBufferSource();
  src.buffer = noiseBuf;
  src.loop = true;
  const f = a.createBiquadFilter();
  const cfg: Record<string, [BiquadFilterType, number, number]> = { wind: ["bandpass", 500, 0.3], rain: ["highpass", 2500, 0.5], town: ["bandpass", 900, 0.8], cave: ["lowpass", 250, 1], sea: ["lowpass", 600, 0.4] };
  const [type, freq, q] = cfg[kind] ?? cfg.wind;
  f.type = type;
  f.frequency.value = freq;
  f.Q.value = q;
  const g = a.createGain();
  g.gain.value = 0;
  g.gain.linearRampToValueAtTime(fxSettings.bgm * 0.18, a.currentTime + 2);
  // 慢慢起伏（风、浪）。
  const lfo = a.createOscillator();
  const lg = a.createGain();
  lfo.frequency.value = kind === "sea" ? 0.12 : 0.07;
  lg.gain.value = fxSettings.bgm * 0.08;
  lfo.connect(lg).connect(g.gain);
  src.connect(f).connect(g).connect(a.destination);
  src.start();
  lfo.start();
  ambient = {
    kind,
    stop: () => {
      g.gain.linearRampToValueAtTime(0, a.currentTime + 1.2);
      window.setTimeout(() => {
        try {
          src.stop();
          lfo.stop();
        } catch {
          /* 已经停了 */
        }
      }, 1400);
    },
  };
}

/** 按地点和天气猜环境音。 */
export function ambientFor(place: string, weather: string): string {
  if (/雨|暴风/.test(weather)) return "rain";
  if (/海|港|码头|船|岸|湾/.test(place)) return "sea";
  if (/洞|窟|地下|地窖|矿/.test(place)) return "cave";
  if (/镇|城|街|市|酒馆|村/.test(place)) return "town";
  return "wind";
}
