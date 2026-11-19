import { describe, expect, it, vi } from "vitest";
import { MpClient, type ServerMsg, type SocketLike, wsUrl } from "./mpClient";

class FakeSocket implements SocketLike {
  readyState = 0;
  sent: Record<string, unknown>[] = [];
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  constructor(public url: string) {}
  send(d: string) {
    this.sent.push(JSON.parse(d));
  }
  close() {
    this.readyState = 3;
    this.onclose?.({});
  }
  open() {
    this.readyState = 1;
    this.onopen?.({});
  }
  push(m: ServerMsg) {
    this.onmessage?.({ data: JSON.stringify(m) });
  }
}

const room = { id: "r1", name: "灯塔", max_players: 4, players: [], gm_mode: "local", gm_ready: true, state_version: 3 };

describe("联机客户端", () => {
  it("第一次用加入码，之后用令牌和最后序号重连；补发的事件不重复交给界面", () => {
    vi.useFakeTimers();
    const sockets: FakeSocket[] = [];
    const got: ServerMsg[] = [];
    const c = new MpClient({ url: "ws://h/ws", room: "r1", joinCode: "ABC123", name: "阿澜", token: "", lastSeq: 0 }, (m) => got.push(m), () => {}, (u) => {
      const s = new FakeSocket(u);
      sockets.push(s);
      return s;
    });
    c.connect();
    sockets[0].open();
    expect(sockets[0].sent[0]).toMatchObject({ t: "hello", join_code: "ABC123", name: "阿澜", token: "", last_seq: 0 });
    sockets[0].push({ t: "welcome", player: "p1", token: "tok", room });
    expect(c.stateVersion).toBe(3);
    sockets[0].push({ t: "event", seq: 5, kind: "narration", payload: { text: "a" } });
    sockets[0].push({ t: "event", seq: 6, kind: "narration", payload: { text: "b" } });
    // 断线 → 自动重连，带令牌和最后序号，不再带加入码和名字。
    sockets[0].close();
    expect(c.status).toBe("reconnecting");
    vi.advanceTimersByTime(1100);
    sockets[1].open();
    expect(sockets[1].sent[0]).toMatchObject({ t: "hello", join_code: "", name: "", token: "tok", last_seq: 6 });
    sockets[1].push({ t: "welcome", player: "p1", token: "", room });
    sockets[1].push({ t: "event", seq: 6, kind: "narration", payload: { text: "b" } });
    sockets[1].push({ t: "event", seq: 7, kind: "narration", payload: { text: "c" } });
    const seqs = got.filter((m) => m.t === "event").map((m) => (m as { seq: number }).seq);
    expect(seqs).toEqual([5, 6, 7]);
    c.close();
    vi.useRealTimers();
  });

  it("每个行动有唯一编号并带上当前状态版本；私人 GM 对话走单独的消息", () => {
    const s = new FakeSocket("x");
    const c = new MpClient({ url: "ws://h/ws", room: "r1", joinCode: "", name: "a", token: "t", lastSeq: 0 }, () => {}, () => {}, () => s);
    c.connect();
    s.open();
    s.push({ t: "welcome", player: "p1", token: "", room });
    const a = c.act("推门");
    const b = c.act("看信", { private: true });
    expect(a && b && a !== b).toBeTruthy();
    expect(s.sent[1]).toMatchObject({ t: "action", action_id: a, state_version: 3, text: "推门" });
    expect(s.sent[2]).toMatchObject({ t: "private_gm", action_id: b, state_version: 3 });
    s.push({ t: "action_status", action_id: a!, status: "done", reason: "", state_version: 4 });
    expect(c.stateVersion).toBe(4);
    c.close();
  });

  it("身份被拒绝时不再重连", () => {
    vi.useFakeTimers();
    const sockets: FakeSocket[] = [];
    const c = new MpClient({ url: "ws://h/ws", room: "r1", joinCode: "bad", name: "a", token: "", lastSeq: 0 }, () => {}, () => {}, (u) => {
      const s = new FakeSocket(u);
      sockets.push(s);
      return s;
    });
    c.connect();
    sockets[0].open();
    sockets[0].push({ t: "error", code: "unauthorized", message: "加入码不对" });
    sockets[0].close();
    vi.advanceTimersByTime(60000);
    expect(sockets.length).toBe(1);
    expect(c.status).toBe("closed");
    vi.useRealTimers();
  });

  it("地址写法都能转成 WebSocket 地址", () => {
    expect(wsUrl("https://play.example.com")).toBe("wss://play.example.com/ws");
    expect(wsUrl("play.example.com")).toBe("wss://play.example.com/ws");
    expect(wsUrl("192.168.1.5:7788")).toBe("ws://192.168.1.5:7788/ws");
    expect(wsUrl("ws://127.0.0.1:7788/ws", "agent")).toBe("ws://127.0.0.1:7788/agent");
  });
});
