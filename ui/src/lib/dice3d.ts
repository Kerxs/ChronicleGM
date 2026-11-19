// 3D 物理骰子（three.js 渲染 + cannon-es 物理，按需加载）。
// 结果由规则引擎先算好；这里只负责演出，并且保证落地后朝上的点数 = 真实点数：
//   1. 先在内存里把这次投掷完整模拟一遍（随机的出手速度与旋转），记下每一帧的位置和朝向；
//   2. 看最后哪一面朝上（d4 看压在下面的那一面），把真实点数印在那一面，其余面印其他点数；
//   3. 再按记录的帧回放。整个过程没有中途"掰"骰子，看到的翻滚就是物理模拟的翻滚。
// 支持 d4 / d6 / d8 / d10 / d12 / d20；其他面数（d2、d100……）由调用方用平面骰子显示。
import * as THREE from "three";
import * as CANNON from "cannon-es";

export const SUPPORTED_SIDES = [4, 6, 8, 10, 12, 20];

export interface DieSpec {
  sides: number;
  /** 规则引擎给出的真实点数。 */
  value: number;
  /** 骰子颜色（不同用途不同颜色）。 */
  color: string;
}

type Face = THREE.Vector3[];

/** 各种骰子的面（每个面是按外侧逆时针排列的顶点）。 */
function faces(sides: number): Face[] {
  const v = (x: number, y: number, z: number) => new THREE.Vector3(x, y, z);
  switch (sides) {
    case 4: {
      const p = [v(1, 1, 1), v(-1, -1, 1), v(-1, 1, -1), v(1, -1, -1)];
      return [[p[0], p[1], p[2]], [p[0], p[3], p[1]], [p[0], p[2], p[3]], [p[1], p[3], p[2]]];
    }
    case 6: {
      const c = (x: number, y: number, z: number) => v(x, y, z);
      return [
        [c(1, -1, -1), c(1, 1, -1), c(1, 1, 1), c(1, -1, 1)],
        [c(-1, -1, 1), c(-1, 1, 1), c(-1, 1, -1), c(-1, -1, -1)],
        [c(-1, 1, -1), c(-1, 1, 1), c(1, 1, 1), c(1, 1, -1)],
        [c(-1, -1, 1), c(-1, -1, -1), c(1, -1, -1), c(1, -1, 1)],
        [c(-1, -1, 1), c(1, -1, 1), c(1, 1, 1), c(-1, 1, 1)],
        [c(1, -1, -1), c(-1, -1, -1), c(-1, 1, -1), c(1, 1, -1)],
      ];
    }
    case 8: {
      const px = v(1, 0, 0), nx = v(-1, 0, 0), py = v(0, 1, 0), ny = v(0, -1, 0), pz = v(0, 0, 1), nz = v(0, 0, -1);
      return [[px, py, pz], [py, nx, pz], [nx, ny, pz], [ny, px, pz], [py, px, nz], [nx, py, nz], [ny, nx, nz], [px, ny, nz]];
    }
    case 10: {
      // 五角偏方面体：上下两个尖，中间两圈错开的点。
      const top = v(0, 1, 0), bot = v(0, -1, 0);
      const h = 0.105;
      const up: THREE.Vector3[] = [], dn: THREE.Vector3[] = [];
      for (let i = 0; i < 5; i++) {
        const a = (i * 2 * Math.PI) / 5, b = a + Math.PI / 5;
        up.push(v(Math.cos(a), h, Math.sin(a)));
        dn.push(v(Math.cos(b), -h, Math.sin(b)));
      }
      const out: Face[] = [];
      for (let i = 0; i < 5; i++) {
        out.push([top, up[(i + 1) % 5], dn[i], up[i]]);
        out.push([bot, dn[i], up[(i + 1) % 5], dn[(i + 1) % 5]]);
      }
      return out;
    }
    case 12: {
      const g = new THREE.DodecahedronGeometry(1);
      return groupCoplanar(g);
    }
    default: {
      const g = new THREE.IcosahedronGeometry(1);
      return groupCoplanar(g);
    }
  }
}

/** 把 three 自带几何体的三角形按共面合并成多边形面（十二面体的五边形）。 */
function groupCoplanar(g: THREE.BufferGeometry): Face[] {
  const pos = g.getAttribute("position");
  const tris: { n: THREE.Vector3; pts: THREE.Vector3[] }[] = [];
  for (let i = 0; i < pos.count; i += 3) {
    const a = new THREE.Vector3().fromBufferAttribute(pos, i), b = new THREE.Vector3().fromBufferAttribute(pos, i + 1), c = new THREE.Vector3().fromBufferAttribute(pos, i + 2);
    const n = new THREE.Vector3().subVectors(b, a).cross(new THREE.Vector3().subVectors(c, a)).normalize();
    tris.push({ n, pts: [a, b, c] });
  }
  const groups: { n: THREE.Vector3; pts: THREE.Vector3[] }[] = [];
  for (const t of tris) {
    const gr = groups.find((x) => x.n.dot(t.n) > 0.999);
    if (gr) gr.pts.push(...t.pts);
    else groups.push({ n: t.n.clone(), pts: [...t.pts] });
  }
  return groups.map((gr) => {
    // 去重后按法线方向绕中心排序成凸多边形。
    const uniq: THREE.Vector3[] = [];
    for (const p of gr.pts) if (!uniq.some((q) => q.distanceTo(p) < 1e-4)) uniq.push(p);
    const c = uniq.reduce((s, p) => s.add(p), new THREE.Vector3()).divideScalar(uniq.length);
    const u = new THREE.Vector3().subVectors(uniq[0], c).normalize();
    const w = new THREE.Vector3().crossVectors(gr.n, u);
    return uniq.sort((p, q) => Math.atan2(new THREE.Vector3().subVectors(p, c).dot(w), new THREE.Vector3().subVectors(p, c).dot(u)) - Math.atan2(new THREE.Vector3().subVectors(q, c).dot(w), new THREE.Vector3().subVectors(q, c).dot(u)));
  });
}

function normalOf(f: Face): THREE.Vector3 {
  const n = new THREE.Vector3().subVectors(f[1], f[0]).cross(new THREE.Vector3().subVectors(f[2], f[0])).normalize();
  const c = f.reduce((s, p) => s.clone().add(p), new THREE.Vector3());
  return n.dot(c) < 0 ? n.negate() : n;
}

/** 让每个面都按外侧逆时针（cannon 的凸多面体需要）。 */
function orient(fs: Face[]): Face[] {
  return fs.map((f) => {
    const n = new THREE.Vector3().subVectors(f[1], f[0]).cross(new THREE.Vector3().subVectors(f[2], f[0]));
    const c = f.reduce((s, p) => s.clone().add(p), new THREE.Vector3());
    return n.dot(c) < 0 ? [...f].reverse() : f;
  });
}

const SCALE: Record<number, number> = { 4: 0.62, 6: 0.48, 8: 0.6, 10: 0.6, 12: 0.58, 20: 0.62 };

const textureCache = new Map<string, THREE.CanvasTexture>();
function faceTexture(label: string, color: string): THREE.CanvasTexture {
  const key = `${label}|${color}`;
  const hit = textureCache.get(key);
  if (hit) return hit;
  const cv = document.createElement("canvas");
  cv.width = cv.height = 128;
  const g = cv.getContext("2d")!;
  g.fillStyle = color;
  g.fillRect(0, 0, 128, 128);
  g.fillStyle = "#ffffff";
  g.font = `bold ${label.length > 1 ? 50 : 60}px system-ui, sans-serif`;
  g.textAlign = "center";
  g.textBaseline = "middle";
  g.shadowColor = "rgba(0,0,0,.35)";
  g.shadowBlur = 4;
  g.fillText(label, 64, 68);
  if (label === "6" || label === "9") g.fillRect(44, 100, 40, 5); // 区分 6 和 9
  const t = new THREE.CanvasTexture(cv);
  t.colorSpace = THREE.SRGBColorSpace;
  t.anisotropy = 4;
  textureCache.set(key, t);
  return t;
}

/** 一颗骰子的几何体：每个面一个材质组，UV 让数字居中。 */
function dieGeometry(fs: Face[], scale: number): THREE.BufferGeometry {
  const pos: number[] = [], uv: number[] = [], nor: number[] = [];
  const g = new THREE.BufferGeometry();
  let start = 0;
  fs.forEach((f, fi) => {
    const n = normalOf(f);
    const c = f.reduce((s, p) => s.clone().add(p), new THREE.Vector3()).divideScalar(f.length);
    const u = new THREE.Vector3().subVectors(f[0], c).normalize();
    const w = new THREE.Vector3().crossVectors(n, u);
    const r = Math.max(...f.map((p) => p.distanceTo(c))) * 1.25;
    const uvOf = (p: THREE.Vector3) => {
      const d = new THREE.Vector3().subVectors(p, c);
      return [0.5 + d.dot(w) / (2 * r), 0.5 + d.dot(u) / (2 * r)];
    };
    for (let i = 1; i < f.length - 1; i++) {
      for (const p of [f[0], f[i], f[i + 1]]) {
        pos.push(p.x * scale, p.y * scale, p.z * scale);
        nor.push(n.x, n.y, n.z);
        uv.push(...uvOf(p));
      }
    }
    const count = (f.length - 2) * 3;
    g.addGroup(start, count, fi);
    start += count;
  });
  g.setAttribute("position", new THREE.Float32BufferAttribute(pos, 3));
  g.setAttribute("normal", new THREE.Float32BufferAttribute(nor, 3));
  g.setAttribute("uv", new THREE.Float32BufferAttribute(uv, 2));
  return g;
}

function cannonShape(fs: Face[], scale: number): CANNON.ConvexPolyhedron {
  const verts: THREE.Vector3[] = [];
  const idx = (p: THREE.Vector3) => {
    let i = verts.findIndex((q) => q.distanceTo(p) < 1e-4);
    if (i < 0) {
      verts.push(p);
      i = verts.length - 1;
    }
    return i;
  };
  const faceIdx = orient(fs).map((f) => f.map(idx));
  return new CANNON.ConvexPolyhedron({ vertices: verts.map((p) => new CANNON.Vec3(p.x * scale, p.y * scale, p.z * scale)), faces: faceIdx });
}

interface Track {
  frames: { p: THREE.Vector3; q: THREE.Quaternion }[];
}

/** 模拟一次投掷（只算物理，不画），返回每一帧和最后哪一面朝上 / 朝下。 */
function simulate(specs: { fs: Face[]; scale: number }[], bounds: { w: number; h: number }): { tracks: Track[]; up: number[]; down: number[]; flat: number[] } {
  const world = new CANNON.World({ gravity: new CANNON.Vec3(0, -30, 0) });
  world.allowSleep = true;
  const dieMat = new CANNON.Material("die");
  const floorMat = new CANNON.Material("floor");
  world.addContactMaterial(new CANNON.ContactMaterial(dieMat, floorMat, { friction: 0.35, restitution: 0.35 }));
  world.addContactMaterial(new CANNON.ContactMaterial(dieMat, dieMat, { friction: 0.2, restitution: 0.4 }));
  const plane = (pos: CANNON.Vec3, axis: CANNON.Vec3, angle: number) => {
    const b = new CANNON.Body({ mass: 0, material: floorMat, shape: new CANNON.Plane() });
    b.position.copy(pos);
    b.quaternion.setFromAxisAngle(axis, angle);
    world.addBody(b);
  };
  plane(new CANNON.Vec3(0, 0, 0), new CANNON.Vec3(1, 0, 0), -Math.PI / 2);
  plane(new CANNON.Vec3(-bounds.w, 0, 0), new CANNON.Vec3(0, 1, 0), Math.PI / 2);
  plane(new CANNON.Vec3(bounds.w, 0, 0), new CANNON.Vec3(0, 1, 0), -Math.PI / 2);
  plane(new CANNON.Vec3(0, 0, -bounds.h), new CANNON.Vec3(0, 1, 0), 0);
  plane(new CANNON.Vec3(0, 0, bounds.h), new CANNON.Vec3(0, 1, 0), Math.PI);
  const bodies = specs.map((s, i) => {
    const b = new CANNON.Body({ mass: 1, material: dieMat, shape: cannonShape(s.fs, s.scale), sleepSpeedLimit: 0.04, sleepTimeLimit: 0.6, angularDamping: 0.12, linearDamping: 0.05 });
    b.position.set(-bounds.w + 0.8 + (i % 3) * 0.7, 2.2 + Math.floor(i / 3) * 0.9, (Math.random() - 0.5) * bounds.h);
    b.quaternion.setFromEuler(Math.random() * 6.28, Math.random() * 6.28, Math.random() * 6.28);
    b.velocity.set(9 + Math.random() * 5, 1 + Math.random() * 2, (Math.random() - 0.5) * 6);
    b.angularVelocity.set((Math.random() - 0.5) * 30, (Math.random() - 0.5) * 30, (Math.random() - 0.5) * 30);
    world.addBody(b);
    return b;
  });
  const tracks: Track[] = specs.map(() => ({ frames: [] }));
  const dt = 1 / 60;
  const upv = new THREE.Vector3(0, 1, 0);
  // 平放：压在桌面上的那一面朝正下方（允许约 10° 误差）。
  const flatness = (i: number) => {
    const b = bodies[i];
    const q = new THREE.Quaternion(b.quaternion.x, b.quaternion.y, b.quaternion.z, b.quaternion.w);
    return -Math.min(...specs[i].fs.map((f) => normalOf(f).applyQuaternion(q).dot(upv)));
  };
  for (let step = 0; step < 60 * 9; step++) {
    world.step(dt);
    bodies.forEach((b, i) => tracks[i].frames.push({ p: new THREE.Vector3(b.position.x, b.position.y, b.position.z), q: new THREE.Quaternion(b.quaternion.x, b.quaternion.y, b.quaternion.z, b.quaternion.w) }));
    const allAsleep = bodies.every((b) => b.sleepState === CANNON.Body.SLEEPING);
    // 靠着墙来回晃、一直睡不着的：每秒检查一次，慢下来了却没平放也推一下。
    const stuck = step > 150 && step % 60 === 0;
    if ((step > 90 && allAsleep) || stuck) {
      // 停下了，但有骰子靠在墙上或别的骰子上（没有平放）：轻轻推一下，继续模拟（回放时只是多晃一下）。
      const tilted = bodies.map((_, i) => i).filter((i) => flatness(i) < 0.985 && (allAsleep || bodies[i].velocity.length() < 0.8));
      if (allAsleep && !tilted.length) break;
      for (const i of tilted) {
        const b = bodies[i];
        b.wakeUp();
        b.velocity.set(-Math.sign(b.position.x) * (1 + Math.random()), 2.5, -Math.sign(b.position.z) * (1 + Math.random()));
        b.angularVelocity.set((Math.random() - 0.5) * 8, (Math.random() - 0.5) * 8, (Math.random() - 0.5) * 8);
      }
    }
  }
  // 时间到了还靠着的（很少见，多是 d4）：用 20 帧倒向离得最近的那一面平放（倾角通常不到 20°，看起来就是倒下）。
  specs.forEach((s, i) => {
    if (flatness(i) >= 0.985) return;
    const fr = tracks[i].frames;
    const last = fr[fr.length - 1];
    const normals = s.fs.map((f) => normalOf(f));
    const world = normals.map((n) => n.clone().applyQuaternion(last.q));
    const k = world.reduce((best, n, j) => (n.y < world[best].y ? j : best), 0);
    const fix = new THREE.Quaternion().setFromUnitVectors(world[k], new THREE.Vector3(0, -1, 0));
    const endQ = fix.multiply(last.q.clone());
    const c = s.fs[k].reduce((acc, p) => acc.clone().add(p), new THREE.Vector3()).divideScalar(s.fs[k].length);
    const restY = c.dot(normals[k]) * s.scale;
    for (let t = 1; t <= 20; t++) {
      const e = t / 20;
      fr.push({ p: new THREE.Vector3(last.p.x, last.p.y + (restY - last.p.y) * e, last.p.z), q: last.q.clone().slerp(endQ, e * e) });
    }
    const b = bodies[i];
    b.quaternion.set(endQ.x, endQ.y, endQ.z, endQ.w);
  });
  // 各条轨迹补齐到同样长（先停下的骰子保持最后的姿势）。
  const longest = Math.max(...tracks.map((t) => t.frames.length));
  for (const t of tracks) while (t.frames.length < longest) t.frames.push(t.frames[t.frames.length - 1]);
  const up: number[] = [], down: number[] = [], flat: number[] = [];
  specs.forEach((s, i) => {
    const q = tracks[i].frames[tracks[i].frames.length - 1].q;
    const dots = s.fs.map((f) => normalOf(f).applyQuaternion(q).dot(upv));
    up.push(dots.indexOf(Math.max(...dots)));
    down.push(dots.indexOf(Math.min(...dots)));
    // 平放程度：压在桌面上的那一面法线和"向下"的夹角余弦（1 = 完全平放）。
    flat.push(-Math.min(...dots));
  });
  return { tracks, up, down, flat };
}

/** 给面分配点数：落地朝上的面（d4 是压在下面那面之外的三个面）印真实点数，其余面印其他点数。 */
export function labelFaces(sides: number, value: number, upFace: number, downFace: number): string[] {
  const others = Array.from({ length: sides }, (_, i) => i + 1).filter((n) => n !== value);
  const labels: string[] = [];
  let k = 0;
  for (let f = 0; f < sides; f++) {
    if (sides === 4 ? f !== downFace : f === upFace) labels.push(String(value));
    else labels.push(String(others[k++ % others.length]));
  }
  if (sides === 10) return labels.map((l) => (l === "10" ? "0" : l));
  return labels;
}

/** 只做物理与贴数字、不画（测试用）：返回每颗骰子最后朝上那面（d4 是朝下那面之外）印的数字。 */
export function settleForTest(dice: DieSpec[]): { shown: string[]; flat: number }[] {
  const specs = dice.map((d) => ({ fs: faces(d.sides), scale: SCALE[d.sides] ?? 0.6 }));
  const sim = simulate(specs, { w: 5, h: 3 });
  return dice.map((d, i) => {
    const labels = labelFaces(d.sides, d.value, sim.up[i], sim.down[i]);
    return { shown: d.sides === 4 ? labels.filter((_, f) => f !== sim.down[i]) : [labels[sim.up[i]]], flat: sim.flat[i] };
  });
}

/** 一张 3D 骰子桌：投出一组骰子，播完后回调。 */
export class DiceTable {
  private renderer: THREE.WebGLRenderer;
  private scene = new THREE.Scene();
  private camera: THREE.PerspectiveCamera;
  private meshes: THREE.Mesh[] = [];
  private raf = 0;
  private timer = 0;
  private bounds = { w: 5, h: 3 };

  constructor(private canvas: HTMLCanvasElement) {
    this.renderer = new THREE.WebGLRenderer({ canvas, alpha: true, antialias: true });
    this.renderer.setPixelRatio(Math.min(2, window.devicePixelRatio || 1));
    this.renderer.shadowMap.enabled = true;
    this.camera = new THREE.PerspectiveCamera(36, 1, 0.1, 100);
    this.camera.position.set(0, 13, 6.5);
    this.camera.lookAt(0, 0, 0);
    this.scene.add(new THREE.HemisphereLight(0xffffff, 0x8899bb, 1.4));
    const sun = new THREE.DirectionalLight(0xffffff, 1.6);
    sun.position.set(-4, 10, 5);
    sun.castShadow = true;
    sun.shadow.mapSize.set(1024, 1024);
    Object.assign(sun.shadow.camera, { left: -8, right: 8, top: 8, bottom: -8 });
    this.scene.add(sun);
    const floor = new THREE.Mesh(new THREE.PlaneGeometry(40, 40), new THREE.ShadowMaterial({ opacity: 0.18 }));
    floor.rotation.x = -Math.PI / 2;
    floor.receiveShadow = true;
    this.scene.add(floor);
    this.resize();
  }

  resize() {
    const w = this.canvas.clientWidth || 1, h = this.canvas.clientHeight || 1;
    this.renderer.setSize(w, h, false);
    this.camera.aspect = w / h;
    this.camera.updateProjectionMatrix();
    // 桌面范围随画面比例变化，骰子不会滚出画面。
    this.bounds = { w: Math.max(3, 4.2 * Math.min(1.8, w / h)), h: 2.6 };
  }

  /** 投掷；`onSettled` 在全部骰子停下时调用（回放结束）。 */
  throw(dice: DieSpec[], onSettled: () => void) {
    cancelAnimationFrame(this.raf);
    clearInterval(this.timer);
    for (const m of this.meshes) {
      this.scene.remove(m);
      m.geometry.dispose();
    }
    this.meshes = [];
    const specs = dice.map((d) => ({ fs: faces(d.sides), scale: SCALE[d.sides] ?? 0.6 }));
    const sim = simulate(specs, this.bounds);
    dice.forEach((d, i) => {
      const labels = labelFaces(d.sides, d.value, sim.up[i], sim.down[i]);
      const mats = labels.map((l) => new THREE.MeshStandardMaterial({ map: faceTexture(l, d.color), roughness: 0.35, metalness: 0.05 }));
      const mesh = new THREE.Mesh(dieGeometry(specs[i].fs, specs[i].scale), mats);
      mesh.castShadow = true;
      this.scene.add(mesh);
      this.meshes.push(mesh);
    });
    const len = Math.max(...sim.tracks.map((t) => t.frames.length));
    const t0 = performance.now();
    let done = false;
    const tick = () => {
      if (done) return;
      // 按时间取帧（和刷新率无关；卡顿或窗口在后台时直接跳到对应帧）。
      const f = Math.min(len - 1, Math.floor(((performance.now() - t0) / 1000) * 60));
      sim.tracks.forEach((t, i) => {
        const fr = t.frames[Math.min(f, t.frames.length - 1)];
        this.meshes[i].position.copy(fr.p);
        this.meshes[i].quaternion.copy(fr.q);
      });
      this.renderer.render(this.scene, this.camera);
      if (f >= len - 1) {
        done = true;
        clearInterval(this.timer);
        onSettled();
        return;
      }
      this.raf = requestAnimationFrame(tick);
    };
    // 动画帧负责流畅；计时器兜底：窗口被遮挡时浏览器会暂停动画帧，演出仍按时间走完，不会卡住后面的掷骰。
    clearInterval(this.timer);
    this.timer = window.setInterval(tick, 100);
    this.raf = requestAnimationFrame(tick);
  }

  dispose() {
    cancelAnimationFrame(this.raf);
    clearInterval(this.timer);
    for (const m of this.meshes) m.geometry.dispose();
    this.renderer.dispose();
  }
}
