// 玻璃球：和 Chronicle GM 大厅里的同一个 WebGL 着色器（缓慢自转、内部粒子流动、菲涅尔边缘光、折射、跟随指针的高光）。
// 安装 / 卸载进行中时转得更快、更亮（和 GM 构思时一样）；不支持 WebGL 时显示静态球。
(function () {
  const VERT = `attribute vec2 p; varying vec2 uv; void main(){ uv = p; gl_Position = vec4(p, 0.0, 1.0); }`;
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
  const cv = document.getElementById("orb");
  const live = { energy: 0, busy: false, hover: 0, ripple: 1, mouse: [0, 0] };
  window.orbBusy = (on) => { live.busy = on; };
  const gl = cv.getContext("webgl", { premultipliedAlpha: false, antialias: true, alpha: true });
  const fail = () => { cv.style.display = "none"; document.getElementById("orb-static").style.display = "block"; };
  if (!gl) return fail();
  const sh = (type, s) => { const o = gl.createShader(type); gl.shaderSource(o, s); gl.compileShader(o); if (!gl.getShaderParameter(o, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(o)); return o; };
  let prog;
  try {
    prog = gl.createProgram();
    gl.attachShader(prog, sh(gl.VERTEX_SHADER, VERT));
    gl.attachShader(prog, sh(gl.FRAGMENT_SHADER, FRAG));
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog));
  } catch (e) { console.warn(e); return fail(); }
  gl.useProgram(prog);
  const buf = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(prog, "p");
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
  gl.enable(gl.BLEND);
  gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
  const U = {}; ["t", "energy", "ripple", "aspect", "mouse", "ca", "cb"].forEach((n) => (U[n] = gl.getUniformLocation(prog, n)));
  // 和软件一致的默认白蓝主题。
  const A = [0.36, 0.6, 1.0], B = [0.78, 0.55, 1.0];
  const t0 = performance.now(); let last = t0;
  const draw = (now) => {
    requestAnimationFrame(draw);
    if (document.hidden) return;
    const dt = Math.min(0.05, (now - last) / 1000); last = now;
    const goal = Math.max(live.hover * 0.6, live.busy ? 1 : 0);
    live.energy += (goal - live.energy) * (1 - Math.exp(-dt * 3));
    live.ripple = Math.min(1, live.ripple + dt * 0.9);
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    const w = Math.round(cv.clientWidth * dpr), h = Math.round(cv.clientHeight * dpr);
    if (cv.width !== w || cv.height !== h) { cv.width = w; cv.height = h; }
    gl.viewport(0, 0, w, h); gl.clearColor(0, 0, 0, 0); gl.clear(gl.COLOR_BUFFER_BIT);
    gl.uniform1f(U.t, (now - t0) / 1000); gl.uniform1f(U.energy, live.energy); gl.uniform1f(U.ripple, live.ripple);
    gl.uniform1f(U.aspect, w / Math.max(1, h)); gl.uniform2f(U.mouse, live.mouse[0], live.mouse[1]);
    gl.uniform3f(U.ca, A[0], A[1], A[2]); gl.uniform3f(U.cb, B[0], B[1], B[2]);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
  };
  requestAnimationFrame(draw);
  cv.addEventListener("pointermove", (e) => { const r = cv.getBoundingClientRect(); live.mouse = [((e.clientX - r.left) / r.width) * 2 - 1, 1 - ((e.clientY - r.top) / r.height) * 2]; });
  cv.addEventListener("pointerenter", () => (live.hover = 1));
  cv.addEventListener("pointerleave", () => { live.hover = 0; live.mouse = [0, 0]; });
  cv.addEventListener("click", () => (live.ripple = 0));
})();
