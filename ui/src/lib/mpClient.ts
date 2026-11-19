// 联机客户端：一条 WebSocket 连接到联机服务端（云服务器 wss://域名/ws，或房主主机 ws://局域网地址:端口/ws）。
// - 只发意图（行动文字 / 战斗按钮 / 聊天），结果全部由服务端算好再广播；
// - 断线自动重连：带上会话令牌和看到的最后一个事件序号，服务端只补发错过的；
// - 每个行动有唯一编号，重连后重发也不会执行两次；
// - 事件按序号去重（重连补发和实时广播可能重叠）。

export type Visibility = { scope: "public" } | { scope: "personal"; player: string } | { scope: "players"; players: string[] } | { scope: "gm_only" };

export interface PlayerInfo {
  id: string;
  name: string;
  character: string;
  host: boolean;
  online: boolean;
}
export interface RoomInfo {
  id: string;
  name: string;
  max_players: number;
  players: PlayerInfo[];
  gm_mode: string;
  gm_ready: boolean;
  state_version: number;
}
export interface GmStatus {
  mode: string;
  active: string;
  ready: boolean;
  agent_online: boolean;
  agent_model: string;
  cloud_model: string;
  cloud_has_key: boolean;
}
export interface CloudGmConfig {
  base_url: string;
  model: string;
  api_key: string;
  temperature: number;
  context_length: number;
  timeout_secs: number;
  max_tokens: number;
}
export interface CombatIntent {
  kind: string;
  target?: string;
  item?: string;
  skill?: string;
  x?: number | null;
  y?: number | null;
}
export type ActionState = "queued" | "running" | "done" | "rejected";

export type ServerMsg =
  | { t: "welcome"; player: string; token: string; room: RoomInfo }
  | { t: "room"; room: RoomInfo }
  | { t: "event"; seq: number; kind: string; payload: Record<string, unknown> }
  | { t: "chat"; from: string; to: string[]; text: string; at: number }
  | { t: "action_status"; action_id: string; status: ActionState; reason: string; state_version: number }
  | { t: "error"; code: string; message: string }
  | { t: "pong"; nonce: number }
  | { t: "snapshot"; state_version: number; data: Record<string, unknown> }
  | { t: "gm"; gm: GmStatus }
  | { t: "dice_ask"; [k: string]: unknown }
  | { t: "gm_stage"; [k: string]: unknown };

export const PROTOCOL_VERSION = 1;

export interface Session {
  url: string;
  room: string;
  /** 第一次加入用加入码；之后用会话令牌重连。 */
  joinCode: string;
  name: string;
  token: string;
  lastSeq: number;
}

/** 浏览器 WebSocket 的最小接口（测试里换成假的）。 */
export interface SocketLike {
  readyState: number;
  send(data: string): void;
  close(): void;
  onopen: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onclose: ((ev: unknown) => void) | null;
  onerror: ((ev: unknown) => void) | null;
}

export type Status = "connecting" | "online" | "reconnecting" | "closed";

const SAVE_KEY = (room: string) => `mp.session.${room}`;

export function loadSession(room: string): Session | null {
  try {
    const raw = localStorage.getItem(SAVE_KEY(room));
    return raw ? (JSON.parse(raw) as Session) : null;
  } catch {
    return null;
  }
}

function saveSession(s: Session) {
  try {
    localStorage.setItem(SAVE_KEY(s.room), JSON.stringify(s));
  } catch {
    // 存不下也能玩，只是重启软件后要重新输入加入码。
  }
}

export function newActionId(): string {
  const c = globalThis.crypto as Crypto | undefined;
  if (c?.randomUUID) return c.randomUUID();
  return `a${Date.now().toString(36)}${Math.random().toString(36).slice(2, 10)}`;
}

export class MpClient {
  session: Session;
  status: Status = "connecting";
  me = "";
  room: RoomInfo | null = null;
  gm: GmStatus | null = null;
  stateVersion = 0;
  private ws: SocketLike | null = null;
  private seen = new Set<number>();
  private backoff = 1000;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private ping: ReturnType<typeof setInterval> | null = null;
  private stopped = false;
  private fatal = false;

  constructor(
    session: Session,
    private onMsg: (m: ServerMsg) => void,
    private onStatus: (s: Status, why?: string) => void,
    private makeSocket: (url: string) => SocketLike = (u) => new WebSocket(u) as unknown as SocketLike,
  ) {
    this.session = { ...session };
  }

  connect() {
    this.stopped = false;
    this.open();
  }

  private open() {
    this.setStatus(this.status === "online" || this.status === "reconnecting" ? "reconnecting" : "connecting");
    let ws: SocketLike;
    try {
      ws = this.makeSocket(this.session.url);
    } catch (e) {
      this.retry(String(e));
      return;
    }
    this.ws = ws;
    ws.onopen = () => {
      const s = this.session;
      ws.send(JSON.stringify({ t: "hello", protocol: PROTOCOL_VERSION, room: s.room, join_code: s.token ? "" : s.joinCode, token: s.token, name: s.token ? "" : s.name, last_seq: s.lastSeq }));
    };
    ws.onmessage = (ev) => {
      let m: ServerMsg;
      try {
        m = JSON.parse(String(ev.data)) as ServerMsg;
      } catch {
        return;
      }
      this.handle(m);
    };
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.ws = null;
      if (this.ping) clearInterval(this.ping);
      this.ping = null;
      if (this.stopped || this.fatal) {
        this.setStatus("closed");
        return;
      }
      this.retry("连接断开");
    };
    ws.onerror = () => {};
  }

  private retry(why: string) {
    this.setStatus("reconnecting", why);
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => this.open(), this.backoff);
    this.backoff = Math.min(this.backoff * 2, 30000);
  }

  private setStatus(s: Status, why?: string) {
    this.status = s;
    this.onStatus(s, why);
  }

  private handle(m: ServerMsg) {
    switch (m.t) {
      case "welcome":
        this.me = m.player;
        this.room = m.room;
        this.stateVersion = m.room.state_version;
        if (m.token) this.session.token = m.token;
        saveSession(this.session);
        this.backoff = 1000;
        this.setStatus("online");
        if (this.ping) clearInterval(this.ping);
        this.ping = setInterval(() => this.send({ t: "ping", nonce: Date.now() }), 25000);
        break;
      case "room":
        this.room = m.room;
        this.stateVersion = Math.max(this.stateVersion, m.room.state_version);
        break;
      case "gm":
        this.gm = m.gm;
        break;
      case "action_status":
        this.stateVersion = Math.max(this.stateVersion, m.state_version);
        break;
      case "snapshot":
        this.stateVersion = Math.max(this.stateVersion, m.state_version);
        break;
      case "event":
        // 重连补发和实时广播可能重叠：同一序号只交给界面一次。
        if (this.seen.has(m.seq)) return;
        this.seen.add(m.seq);
        if (m.seq > this.session.lastSeq) {
          this.session.lastSeq = m.seq;
          saveSession(this.session);
        }
        break;
      case "error":
        // 身份不对 / 房间不存在 / 协议不一致：重连也没用，停下来让用户处理。
        if (["unauthorized", "room_not_found", "bad_protocol", "room_full"].includes(m.code)) this.fatal = true;
        break;
    }
    this.onMsg(m);
  }

  send(m: Record<string, unknown>): boolean {
    if (!this.ws || this.ws.readyState !== 1) return false;
    this.ws.send(JSON.stringify(m));
    return true;
  }

  /** 提交行动（返回行动编号；不在线时返回 null）。 */
  act(text: string, opts: { private?: boolean; combat?: CombatIntent } = {}): string | null {
    const action_id = newActionId();
    const ok = opts.private
      ? this.send({ t: "private_gm", action_id, state_version: this.stateVersion, text })
      : this.send({ t: "action", action_id, state_version: this.stateVersion, text, combat: opts.combat ?? null });
    return ok ? action_id : null;
  }

  chat(text: string, to: string[] = []) {
    return this.send({ t: "chat", to, text });
  }

  getState() {
    return this.send({ t: "get_state" });
  }

  setGm(mode: string, cloud?: CloudGmConfig | null) {
    return this.send({ t: "set_gm", mode, cloud: cloud ?? null });
  }

  close() {
    this.stopped = true;
    if (this.timer) clearTimeout(this.timer);
    if (this.ping) clearInterval(this.ping);
    this.ws?.close();
    this.ws = null;
    this.setStatus("closed");
  }

  /** 离开房间并忘掉令牌（下次要用加入码重新加入）。 */
  forget() {
    try {
      localStorage.removeItem(SAVE_KEY(this.session.room));
    } catch {
      // 忽略
    }
    this.close();
  }
}

/** 人看得懂的连接地址：`https://x` → `wss://x/ws`，`192.168.1.5:7788` → `ws://192.168.1.5:7788/ws`。 */
export function wsUrl(address: string, path: "ws" | "agent" = "ws"): string {
  let a = address.trim().replace(/\/+$/, "");
  if (a.startsWith("https://")) a = "wss://" + a.slice(8);
  else if (a.startsWith("http://")) a = "ws://" + a.slice(7);
  else if (!a.startsWith("ws://") && !a.startsWith("wss://")) a = (/^(\d+\.){3}\d+(:\d+)?$|^localhost(:\d+)?$|:\d+$/.test(a) ? "ws://" : "wss://") + a;
  a = a.replace(/\/(ws|agent)$/, "");
  return `${a}/${path}`;
}
