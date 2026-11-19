// 大厅与游戏顶部的玻璃球（WebGL 片元着色器，GPU 绘制）：
// 缓慢自转、内部粒子沿流场流动、菲涅尔边缘光与背景折射、跟随指针的高光；悬停时粒子加速变亮，点击产生涟漪。
// 选中剧本后按主题色渐变过去（世界反馈）。窗口隐藏、元素不在视口内时停止绘制；低性能模式或不支持 WebGL 时退回静态 CSS 球。
import { useEffect, useRef, useState } from "react";

export type OrbTheme = { a: [number, number, number]; b: [number, number, number] };

/** 默认的白蓝主题（和界面一致）。 */
export const DEFAULT_THEME: OrbTheme = { a: [0.36, 0.6, 1.0], b: [0.78, 0.55, 1.0] };

/** 按题材和名字给剧本一个稳定的主题色。 */
export function themeFor(genre: string, title: string): OrbTheme {
  const g = genre + title;
  const preset: [RegExp, OrbTheme][] = [
    [/恐怖|惊悚|克苏鲁|诡/, { a: [0.55, 0.12, 0.22], b: [0.2, 0.9, 0.55] }],
    [/悬疑|推理|侦探|谜/, { a: [0.25, 0.32, 0.75], b: [0.95, 0.75, 0.35] }],
    [/海|潮|灯塔|港|雾/, { a: [0.15, 0.55, 0.85], b: [0.55, 0.95, 0.9] }],
    [/奇幻|魔法|龙|精灵/, { a: [0.55, 0.3, 0.95], b: [1.0, 0.7, 0.4] }],
    [/科幻|赛博|星|机甲|太空/, { a: [0.1, 0.85, 0.95], b: [0.95, 0.25, 0.75] }],
    [/武侠|江湖|仙|修/, { a: [0.85, 0.3, 0.25], b: [0.95, 0.85, 0.55] }],
    [/末日|废土|生存/, { a: [0.85, 0.55, 0.2], b: [0.45, 0.75, 0.35] }],
  ];
  for (const [re, t] of preset) if (re.test(g)) return t;
  let h = 0;
  for (const ch of g) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  const hue = (h % 360) / 360;
  const hsl = (hh: number, s: number, l: number): [number, number, number] => {
    const f = (n: number) => {
      const k = (n + hh * 12) % 12;
      return l - s * Math.min(l, 1 - l) * Math.max(-1, Math.min(k - 3, 9 - k, 1));
    };
    return [f(0), f(8), f(4)];
  };
  return { a: hsl(hue, 0.75, 0.55), b: hsl((hue + 0.18) % 1, 0.8, 0.65) };
}

const VERT = `attribute vec2 p; varying vec2 uv; void main(){ uv = p; gl_Position = vec4(p, 0.0, 1.0); }`;

// 球内粒子：在旋转后的三维坐标里分层取样，每层按格子放一个随时间沿流场漂移的光点。
const FRAG = `precision highp float;
varying vec2 uv;
uniform float t, energy, ripple, aspect;
uniform vec2 mouse;
uniform vec3 ca, cb;
float h1(vec3 p){ return fract(sin(dot(p, vec3(127.1, 311.7, 74.7))) * 43758.5453); }
vec3 h3(vec3 p){ return fract(sin(vec3(dot(p,vec3(127.1,311.7,74.7)), dot(p,vec3(269.5,183.3,246.1)), dot(p,vec3(113.5,271.9,124.6)))) * 43758.5453); }
mat3 rotY(float a){ float c = cos(a), s = sin(a); return mat3(c,0.,-s, 0.,1.,0., s,0.,c); }
mat3 rotX(float a){ float c = cos(a), s = sin(a); return mat3(1.,0.,0., 0.,c,s, 0.,-s,c); }
float particles(vec3 p, float speed){
  float acc = 0.0;
  for (int L = 0; L < 3; L++) {
    float k = 3.0 + float(L) * 2.5;
    vec3 q = p * k;
    q.y += t * speed * (0.35 + 0.15 * float(L));
    q.x += sin(q.y * 0.7 + t * 0.3) * 0.6;
    vec3 cell = floor(q);
    vec3 f = fract(q);
    vec3 r = h3(cell + float(L) * 17.0);
    vec3 c = 0.2 + 0.6 * vec3(r.x, fract(r.y + t * 0.05 * speed), r.z);
    float d = length(f - c);
    float tw = 0.6 + 0.4 * sin(t * (1.5 + r.x * 2.0) + r.y * 6.28);
    acc += smoothstep(0.16, 0.0, d) * tw * (0.55 + 0.45 * h1(cell));
  }
  return acc;
}
void main(){
  vec2 q = uv; q.x *= aspect;
  float r = length(q);
  float R = 0.82;
  // 背景：柔和的渐变光斑（被球折射）。
  vec3 bg = mix(vec3(0.96, 0.97, 1.0), mix(ca, cb, 0.5) * 0.25 + 0.75, 0.25 * exp(-r * r * 1.5));
  if (r > R + 0.02) {
    float halo = exp(-pow(max(r - R, 0.0) * 9.0, 2.0)) * (0.35 + 0.4 * energy);
    gl_FragColor = vec4(mix(ca, cb, 0.5 + 0.5 * sin(t * 0.4)) , halo * 0.55);
    return;
  }
  vec2 s = q / R;
  float z = sqrt(max(0.0, 1.0 - dot(s, s)));
  vec3 n = vec3(s, z);
  float rot = t * (0.08 + 0.12 * energy);
  mat3 m = rotY(rot) * rotX(0.35 + 0.05 * sin(t * 0.2));
  // 体积：沿视线方向取 6 层。
  float glow = 0.0;
  for (int i = 0; i < 6; i++) {
    float d = (float(i) + 0.5) / 6.0;
    vec3 p = m * vec3(s, z * (1.0 - 2.0 * d));
    glow += particles(p, 0.6 + 1.6 * energy) * (1.0 - d * 0.6);
  }
  glow /= 1.7;
  // 内部的流动色带。
  vec3 pm = m * n;
  float band = 0.5 + 0.5 * sin(pm.y * 5.0 + sin(pm.x * 3.0 + t * 0.5) * 1.5 + t * 0.35);
  vec3 inner = mix(ca, cb, band) * (0.35 + 0.25 * z);
  // 折射：背景按法线偏移后透过来。
  vec2 refr = q - n.xy * 0.18 * (1.0 - z);
  vec3 seen = mix(vec3(0.97, 0.98, 1.0), mix(cb, ca, 0.5 + 0.5 * refr.y), 0.18);
  float fres = pow(1.0 - z, 2.6);
  vec3 col = mix(seen, inner, 0.55) + glow * mix(cb, vec3(1.0), 0.6) * (1.1 + 1.2 * energy);
  col += fres * mix(ca, vec3(1.0), 0.5) * 0.9;
  // 高光跟随指针。
  vec3 l = normalize(vec3(mouse * 0.6 + vec2(-0.35, 0.45), 1.0));
  float spec = pow(max(dot(reflect(-l, n), vec3(0.0, 0.0, 1.0)), 0.0), 48.0);
  col += spec * 1.2;
  col += smoothstep(0.55, 0.0, length(s - vec2(-0.35, 0.42))) * 0.18;
  // 点击涟漪：一圈从中心扩散的亮环。
  float rr = abs(length(s) - ripple * 1.2);
  col += (1.0 - ripple) * smoothstep(0.08, 0.0, rr) * 0.6 * mix(ca, cb, 0.5);
  float edge = smoothstep(R + 0.02, R - 0.01, r);
  gl_FragColor = vec4(col, edge);
}`;

function lerp3(a: [number, number, number], b: [number, number, number], k: number): [number, number, number] {
  return [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k, a[2] + (b[2] - a[2]) * k];
}

export default function OrbCanvas({ theme = DEFAULT_THEME, size, lowPower = false, active = false, onClick, title }: { theme?: OrbTheme; size: number; lowPower?: boolean; active?: boolean; onClick?: () => void; title?: string }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const [failed, setFailed] = useState(false);
  const live = useRef({ theme, target: theme, hover: 0, energy: active ? 1 : 0, active, ripple: 1, mouse: [0, 0] as [number, number] });
  live.current.target = theme;
  live.current.active = active;

  useEffect(() => {
    if (lowPower) return;
    const cv = ref.current;
    if (!cv) return;
    const gl = cv.getContext("webgl", { premultipliedAlpha: false, antialias: true, alpha: true });
    if (!gl) {
      setFailed(true);
      return;
    }
    const sh = (type: number, src: string) => {
      const s = gl.createShader(type)!;
      gl.shaderSource(s, src);
      gl.compileShader(s);
      if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? "shader");
      return s;
    };
    let prog: WebGLProgram;
    try {
      prog = gl.createProgram()!;
      gl.attachShader(prog, sh(gl.VERTEX_SHADER, VERT));
      gl.attachShader(prog, sh(gl.FRAGMENT_SHADER, FRAG));
      gl.linkProgram(prog);
      if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog) ?? "link");
    } catch (e) {
      console.warn("玻璃球着色器不可用，改用静态球：", e);
      setFailed(true);
      return;
    }
    gl.useProgram(prog);
    const buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
    const loc = gl.getAttribLocation(prog, "p");
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    const u = (n: string) => gl.getUniformLocation(prog, n);
    const U = { t: u("t"), energy: u("energy"), ripple: u("ripple"), aspect: u("aspect"), mouse: u("mouse"), ca: u("ca"), cb: u("cb") };

    let visible = true;
    const io = new IntersectionObserver((es) => (visible = es.some((e) => e.isIntersecting)));
    io.observe(cv);
    const t0 = performance.now();
    let last = t0;
    let raf = 0;
    const draw = (now: number) => {
      raf = requestAnimationFrame(draw);
      if (!visible || document.hidden) return;
      const dt = Math.min(0.05, (now - last) / 1000);
      last = now;
      const L = live.current;
      // 主题色、能量、涟漪都按时间平滑过渡（和帧率无关）。
      const k = 1 - Math.exp(-dt * 2.5);
      L.theme = { a: lerp3(L.theme.a, L.target.a, k), b: lerp3(L.theme.b, L.target.b, k) };
      const goal = Math.max(L.hover * 0.6, L.active ? 1 : 0);
      L.energy += (goal - L.energy) * (1 - Math.exp(-dt * 3));
      L.ripple = Math.min(1, L.ripple + dt * 0.9);
      const dpr = Math.min(2, window.devicePixelRatio || 1);
      const w = Math.round(cv.clientWidth * dpr);
      const h = Math.round(cv.clientHeight * dpr);
      if (cv.width !== w || cv.height !== h) {
        cv.width = w;
        cv.height = h;
      }
      gl.viewport(0, 0, w, h);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
      gl.uniform1f(U.t, (now - t0) / 1000);
      gl.uniform1f(U.energy, L.energy);
      gl.uniform1f(U.ripple, L.ripple);
      gl.uniform1f(U.aspect, w / Math.max(1, h));
      gl.uniform2f(U.mouse, L.mouse[0], L.mouse[1]);
      gl.uniform3f(U.ca, ...L.theme.a);
      gl.uniform3f(U.cb, ...L.theme.b);
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    };
    raf = requestAnimationFrame(draw);
    return () => {
      cancelAnimationFrame(raf);
      io.disconnect();
      // 不调用 loseContext：同一个 canvas 可能马上重新挂载（开发模式的 StrictMode、切换低性能模式），
      // 丢掉的上下文拿回来就是坏的（实测：着色器编译失败、退回静态球）。只释放这次创建的 GPU 资源。
      gl.deleteBuffer(buf);
      gl.deleteProgram(prog);
    };
  }, [lowPower]);

  const handlers = {
    onPointerMove: (e: React.PointerEvent) => {
      const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
      live.current.mouse = [((e.clientX - r.left) / r.width) * 2 - 1, 1 - ((e.clientY - r.top) / r.height) * 2];
    },
    onPointerEnter: () => (live.current.hover = 1),
    onPointerLeave: () => {
      live.current.hover = 0;
      live.current.mouse = [0, 0];
    },
    onClick: () => {
      live.current.ripple = 0;
      onClick?.();
    },
  };
  const css = { width: size, height: size, "--orb-a": `rgb(${theme.a.map((x) => Math.round(x * 255)).join(",")})`, "--orb-b": `rgb(${theme.b.map((x) => Math.round(x * 255)).join(",")})` } as React.CSSProperties;
  if (lowPower || failed) {
    return (
      <button type="button" className={`orb-static${active ? " active" : ""}`} style={css} title={title} aria-label={title ?? "GM"} {...handlers}>
        <span className="orb-static-core" />
      </button>
    );
  }
  return <canvas ref={ref} className={`orb-canvas${onClick ? " clickable" : ""}`} style={css} title={title} role={onClick ? "button" : undefined} aria-label={title ?? "GM"} {...handlers} />;
}
