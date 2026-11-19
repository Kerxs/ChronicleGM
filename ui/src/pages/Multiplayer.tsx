// 联机游戏：开房（本机当房主 / 云服务器）、加入房间、房间内的公共聊天 / 私聊 / 私下问 GM / 行动 / 战斗。
// 所有结果都由服务端算好再广播；这里只发意图、显示服务端给的内容（服务端已经按权限过滤过）。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, type ProviderView } from "../api";
import "../mp.css";
import { type CloudGmConfig, type GmStatus, MpClient, type RoomInfo, type ServerMsg, type Session, type Status, loadSession, wsUrl } from "../lib/mpClient";

type Feed = { key: string; kind: "narration" | "npc" | "player" | "roll" | "combat" | "chat" | "whisper" | "system" | "error"; who?: string; text: string };
type Joined = { session: Session; joinCode?: string; agentKey?: string; hostMode?: "host" | "cloud" };

const RECENT_KEY = "mp.recent";

function recentRooms(): Joined[] {
  try {
    return (JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]") as Joined[]).filter((j) => loadSession(j.session.room)).slice(0, 6);
  } catch {
    return [];
  }
}
function remember(j: Joined) {
  try {
    const list = recentRooms().filter((x) => x.session.room !== j.session.room);
    localStorage.setItem(RECENT_KEY, JSON.stringify([{ ...j, agentKey: undefined }, ...list].slice(0, 6)));
  } catch {
    // 忽略
  }
}

export default function Multiplayer({ worldId, notify, onExit }: { worldId: string | null; notify: (m: string, k?: "info" | "error") => void; onExit: () => void }) {
  const [joined, setJoined] = useState<Joined | null>(null);
  return (
    <div className="mp-page">
      <header className="mp-head">
        <button className="link" onClick={onExit}>
          ← 返回大厅
        </button>
        <h2>联机游戏</h2>
        <span className="hint">2–4 人；规则结算都在服务端，客户端改不了生命、骰子和物品。</span>
      </header>
      {joined ? <Room key={joined.session.room} joined={joined} notify={notify} onLeave={() => setJoined(null)} /> : <Setup worldId={worldId} notify={notify} onJoined={(j) => { remember(j); setJoined(j); }} />}
    </div>
  );
}

// ====================== 开房 / 加入 ======================

function Setup({ worldId, notify, onJoined }: { worldId: string | null; notify: (m: string, k?: "info" | "error") => void; onJoined: (j: Joined) => void }) {
  const [tab, setTab] = useState<"join" | "host" | "cloud">("join");
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [name, setName] = useState(() => localStorage.getItem("mp.name") ?? "");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api.listProviders().then((ps) => setProviders(ps.filter((p) => (p.config.service ?? "chat") === "chat"))).catch(() => {});
  }, []);
  useEffect(() => {
    try {
      localStorage.setItem("mp.name", name);
    } catch {
      // 忽略
    }
  }, [name]);
  const recent = useMemo(recentRooms, []);

  // 加入
  const [addr, setAddr] = useState("");
  const [roomId, setRoomId] = useState("");
  const [code, setCode] = useState("");
  // 开房（共用）
  const [roomName, setRoomName] = useState("周末的冒险");
  const [maxPlayers, setMaxPlayers] = useState(4);
  const [provider, setProvider] = useState("");
  // 本机当房主
  const [lan, setLan] = useState(true);
  const [port, setPort] = useState(7788);
  // 云服务器
  const [cloudAddr, setCloudAddr] = useState(() => localStorage.getItem("mp.cloud") ?? "");
  const [adminToken, setAdminToken] = useState("");
  const [gmMode, setGmMode] = useState<"local" | "cloud" | "auto">("local");

  useEffect(() => {
    if (!provider && providers[0]) setProvider(providers[0].config.id);
  }, [providers, provider]);

  const run = async (f: () => Promise<void>) => {
    if (!name.trim()) return notify("先填你的名字", "error");
    setBusy(true);
    try {
      await f();
    } catch (e) {
      notify(e instanceof Error ? e.message : String(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const join = () =>
    run(async () => {
      if (!addr.trim() || !roomId.trim()) throw new Error("填服务器地址和房间号");
      onJoined({ session: { url: wsUrl(addr), room: roomId.trim(), joinCode: code.trim().toUpperCase(), name: name.trim(), token: loadSession(roomId.trim())?.token ?? "", lastSeq: 0 } });
    });

  const hostHere = () =>
    run(async () => {
      const r = await api.mpHostStart({ worldId, lan, port, roomName: roomName.trim(), hostName: name.trim(), maxPlayers, providerId: provider || null });
      notify(`房间已开好。把地址 ${r.addresses[0] ?? `本机:${r.port}`}、房间号 ${r.room_id} 和加入码 ${r.join_code} 发给朋友`);
      onJoined({ session: { url: `ws://127.0.0.1:${r.port}/ws`, room: r.room_id, joinCode: "", name: name.trim(), token: r.host_token, lastSeq: 0 }, joinCode: r.join_code, agentKey: r.agent_key, hostMode: "host" });
    });

  const hostCloud = () =>
    run(async () => {
      if (!cloudAddr.trim() || !adminToken.trim()) throw new Error("填云服务器地址和管理员令牌");
      localStorage.setItem("mp.cloud", cloudAddr.trim());
      const http = wsUrl(cloudAddr, "ws").replace(/^ws/, "http").replace(/\/ws$/, "");
      const r = await api.mpCloudCreateRoom({ url: http, adminToken: adminToken.trim(), roomName: roomName.trim(), hostName: name.trim(), maxPlayers, gmMode });
      if (gmMode !== "cloud" && provider) await api.mpAgentStart({ server: wsUrl(cloudAddr, "agent"), room: r.room_id, agentKey: r.agent_key, providerId: provider });
      onJoined({ session: { url: wsUrl(cloudAddr), room: r.room_id, joinCode: "", name: name.trim(), token: r.host_token, lastSeq: 0 }, joinCode: r.join_code, agentKey: r.agent_key, hostMode: "cloud" });
    });

  const providerPick = (
    <label>
      GM 用的本机模型
      <select value={provider} onChange={(e) => setProvider(e.target.value)}>
        {providers.length === 0 && <option value="">（还没有配置模型服务）</option>}
        {providers.map((p) => (
          <option key={p.config.id} value={p.config.id}>
            {p.config.name} · {p.config.model}
          </option>
        ))}
      </select>
    </label>
  );

  return (
    <div className="mp-setup">
      <div className="lobby-tabs">
        <button className={tab === "join" ? "on" : ""} onClick={() => setTab("join")}>
          加入房间
        </button>
        <button className={tab === "host" ? "on" : ""} onClick={() => setTab("host")}>
          本机当房主
        </button>
        <button className={tab === "cloud" ? "on" : ""} onClick={() => setTab("cloud")}>
          云服务器开房
        </button>
      </div>
      <div className="lobby-card">
        <label>
          你的名字
          <input value={name} maxLength={24} onChange={(e) => setName(e.target.value)} placeholder="阿澜" />
        </label>
        {tab === "join" && (
          <>
            <label>
              服务器地址
              <input value={addr} onChange={(e) => setAddr(e.target.value)} placeholder="play.example.com 或 192.168.1.5:7788" />
            </label>
            <label>
              房间号
              <input value={roomId} onChange={(e) => setRoomId(e.target.value)} />
            </label>
            <label>
              加入码
              <input value={code} maxLength={6} onChange={(e) => setCode(e.target.value)} placeholder="6 位" />
            </label>
            <button className="primary" disabled={busy} onClick={() => void join()}>
              加入
            </button>
            {recent.length > 0 && (
              <div className="mp-recent">
                <small className="hint">最近的房间（断线后可以直接回去）</small>
                {recent.map((j) => (
                  <button key={j.session.room} className="lobby-recent-item" onClick={() => onJoined({ ...j, session: { ...j.session, ...loadSession(j.session.room)! } })}>
                    <strong>{j.session.room}</strong>
                    <small>{j.session.url}</small>
                  </button>
                ))}
              </div>
            )}
          </>
        )}
        {tab === "host" && (
          <>
            <label>
              房间名
              <input value={roomName} onChange={(e) => setRoomName(e.target.value)} />
            </label>
            <label>
              人数上限
              <select value={maxPlayers} onChange={(e) => setMaxPlayers(Number(e.target.value))}>
                {[2, 3, 4].map((n) => (
                  <option key={n} value={n}>
                    {n} 人
                  </option>
                ))}
              </select>
            </label>
            <label className="check">
              <input type="checkbox" checked={lan} onChange={(e) => setLan(e.target.checked)} /> 让同一局域网的朋友连进来
            </label>
            <label>
              端口
              <input type="number" value={port} min={1024} max={65535} onChange={(e) => setPort(Number(e.target.value))} />
            </label>
            {providerPick}
            <p className="hint">世界用当前打开的世界；GM 在你的电脑上调本机模型，模型端口不会暴露给别人。外网朋友要连进来需要你自己做端口转发，或者用云服务器开房。</p>
            <button className="primary" disabled={busy} onClick={() => void hostHere()}>
              开房
            </button>
          </>
        )}
        {tab === "cloud" && (
          <>
            <label>
              云服务器地址
              <input value={cloudAddr} onChange={(e) => setCloudAddr(e.target.value)} placeholder="https://play.example.com" />
            </label>
            <label>
              管理员令牌
              <input type="password" value={adminToken} onChange={(e) => setAdminToken(e.target.value)} placeholder="服务器 /etc/chronicle/server.env 里的" />
            </label>
            <label>
              房间名
              <input value={roomName} onChange={(e) => setRoomName(e.target.value)} />
            </label>
            <label>
              人数上限
              <select value={maxPlayers} onChange={(e) => setMaxPlayers(Number(e.target.value))}>
                {[2, 3, 4].map((n) => (
                  <option key={n} value={n}>
                    {n} 人
                  </option>
                ))}
              </select>
            </label>
            <label>
              AI GM
              <select value={gmMode} onChange={(e) => setGmMode(e.target.value as "local" | "cloud" | "auto")}>
                <option value="local">我电脑上的模型（GM Agent 主动连服务器）</option>
                <option value="cloud">云端模型（进房后填写）</option>
                <option value="auto">自动（我在线用本机，否则用云端）</option>
              </select>
            </label>
            {gmMode !== "cloud" && providerPick}
            <button className="primary" disabled={busy} onClick={() => void hostCloud()}>
              开房
            </button>
          </>
        )}
      </div>
    </div>
  );
}

// ====================== 房间 ======================

function textOf(m: Extract<ServerMsg, { t: "event" }>, room: RoomInfo | null): Feed | null {
  const p = m.payload as Record<string, unknown>;
  const s = (k: string) => (typeof p[k] === "string" ? (p[k] as string) : "");
  const key = `e${m.seq}`;
  switch (m.kind) {
    case "narration":
      return { key, kind: "narration", text: s("text") };
    case "npc":
      return { key, kind: "npc", who: s("name"), text: s("text") };
    case "player":
      return { key, kind: "player", text: s("text") };
    case "roll": {
      const total = p.total ?? p.result ?? "";
      return { key, kind: "roll", text: `🎲 ${s("reason") || s("label") || "掷骰"}：${s("expr") || s("dice")} = ${String(total)}` };
    }
    case "combat_log": {
      const who = room?.players.find((x) => x.id === s("player"))?.name ?? "";
      const lines = Array.isArray(p.lines) ? (p.lines as string[]).join("\n") : "";
      return { key, kind: "combat", who, text: lines };
    }
    default:
      return null;
  }
}

function Room({ joined, notify, onLeave }: { joined: Joined; notify: (m: string, k?: "info" | "error") => void; onLeave: () => void }) {
  const [status, setStatus] = useState<Status>("connecting");
  const [room, setRoom] = useState<RoomInfo | null>(null);
  const [gm, setGm] = useState<GmStatus | null>(null);
  const [me, setMe] = useState("");
  const [feed, setFeed] = useState<Feed[]>([]);
  const [snap, setSnap] = useState<Record<string, unknown> | null>(null);
  const [text, setText] = useState("");
  const [mode, setMode] = useState<string>("act"); // act / private / chat / 玩家 id
  const [pending, setPending] = useState<string | null>(null);
  const client = useRef<MpClient | null>(null);
  const feedEnd = useRef<HTMLDivElement>(null);

  const push = useCallback((f: Feed) => setFeed((v) => (v.some((x) => x.key === f.key) ? v : [...v, f].slice(-400))), []);

  useEffect(() => {
    const c = new MpClient(
      joined.session,
      (m) => {
        switch (m.t) {
          case "welcome":
            setMe(m.player);
            setRoom(m.room);
            c.getState();
            break;
          case "room":
            setRoom(m.room);
            break;
          case "gm":
            setGm(m.gm);
            break;
          case "event": {
            const f = textOf(m, c.room);
            if (f && f.text.trim()) push(f);
            if (m.kind === "combat_log" || m.kind === "turn_end") c.getState();
            break;
          }
          case "chat": {
            const who = c.room?.players.find((x) => x.id === m.from)?.name ?? m.from;
            const whisper = m.to.length > 0;
            const to = whisper ? ` → ${m.to.map((id) => c.room?.players.find((x) => x.id === id)?.name ?? id).join("、")}` : "";
            push({ key: `c${m.at}${m.from}`, kind: whisper ? "whisper" : "chat", who: who + to, text: m.text });
            break;
          }
          case "action_status":
            if (m.status === "done" || m.status === "rejected") {
              setPending((p) => (p === m.action_id ? null : p));
              if (m.status === "rejected" && m.reason) push({ key: `r${m.action_id}`, kind: "error", text: m.reason });
              c.getState();
            }
            break;
          case "snapshot":
            setSnap(m.data);
            break;
          case "error":
            if (m.code !== "duplicate" && m.code !== "stale_version") notify(m.message, "error");
            if (m.code === "stale_version") c.getState();
            break;
        }
      },
      (s, why) => {
        setStatus(s);
        if (s === "reconnecting" && why) push({ key: `s${Date.now()}`, kind: "system", text: `连接断开，正在重连…` });
      },
    );
    client.current = c;
    c.connect();
    return () => c.close();
  }, [joined.session, push, notify]);

  useEffect(() => feedEnd.current?.scrollIntoView({ block: "end" }), [feed.length]);

  const amHost = !!room?.players.find((p) => p.id === me)?.host;
  const others = room?.players.filter((p) => p.id !== me) ?? [];
  const meSnap = (snap?.me ?? {}) as { sheet?: { name?: string; hp?: number; max_hp?: number }; inventory?: { name: string; qty: number; equipped: boolean }[] };
  const combat = (snap?.combat ?? null) as null | { units: { id: string; name: string; side: string; hp: number; max_hp: number; actor?: string | null; out?: string }[]; order: string[]; turn: number; status: string };
  const myActor = `player:${me}`;
  const current = combat ? combat.units.find((u) => u.id === combat.order[combat.turn]) : null;
  const myTurn = !!current && current.actor === myActor;

  const send = () => {
    const c = client.current;
    const t = text.trim();
    if (!c || !t) return;
    if (mode === "act" || mode === "private") {
      const id = c.act(t, { private: mode === "private" });
      if (!id) return notify("还没连上服务器", "error");
      setPending(id);
    } else {
      if (!c.chat(t, mode === "chat" ? [] : [mode])) return notify("还没连上服务器", "error");
    }
    setText("");
  };

  const fight = (kind: string, target = "") => {
    const id = client.current?.act("", { combat: { kind, target } });
    if (id) setPending(id);
  };

  return (
    <div className="mp-room">
      <aside className="mp-side">
        <div className="lobby-card">
          <strong>{room?.name ?? "连接中…"}</strong>
          <small className="hint">
            房间号 {joined.session.room}
            {joined.joinCode ? ` · 加入码 ${joined.joinCode}` : ""}
          </small>
          <span className={`mp-status st-${status}`}>{{ connecting: "连接中", online: "在线", reconnecting: "重连中", closed: "已断开" }[status]}</span>
          <ul className="mp-players">
            {room?.players.map((p) => (
              <li key={p.id} className={p.online ? "on" : ""}>
                <i />
                {p.name}
                {p.host ? " 👑" : ""}
                {p.id === me ? "（你）" : ""}
              </li>
            ))}
          </ul>
          <GmChip gm={gm} />
          <button className="link" onClick={() => { client.current?.close(); onLeave(); }}>
            离开房间
          </button>
        </div>
        {amHost && <GmSettings gm={gm} onSave={(mode, cloud) => client.current?.setGm(mode, cloud)} agentKey={joined.agentKey} />}
      </aside>

      <section className="mp-main">
        <div className="mp-feed">
          {feed.map((f) => (
            <div key={f.key} className={`mp-line k-${f.kind}`}>
              {f.who && <b>{f.who}</b>}
              <span>{f.text}</span>
            </div>
          ))}
          {pending && <div className="mp-line k-system">GM 正在处理…</div>}
          <div ref={feedEnd} />
        </div>
        <div className="mp-input">
          <select value={mode} onChange={(e) => setMode(e.target.value)}>
            <option value="act">行动（大家都看得到）</option>
            <option value="private">私下问 GM（只有我看得到）</option>
            <option value="chat">公共聊天</option>
            {others.map((p) => (
              <option key={p.id} value={p.id}>
                私聊 {p.name}
              </option>
            ))}
          </select>
          <textarea
            value={text}
            maxLength={600}
            rows={2}
            placeholder={mode === "act" ? "你要做什么？" : mode === "private" ? "悄悄告诉 GM…" : "说点什么…"}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                send();
              }
            }}
          />
          <button className="primary" disabled={!text.trim() || status !== "online" || (!!pending && (mode === "act" || mode === "private"))} onClick={send}>
            发送
          </button>
        </div>
      </section>

      <aside className="mp-side">
        <div className="lobby-card">
          <strong>{meSnap.sheet?.name ?? "我的角色"}</strong>
          {meSnap.sheet && (
            <small>
              生命 {meSnap.sheet.hp}/{meSnap.sheet.max_hp}
            </small>
          )}
          <ul className="mp-bag">
            {(meSnap.inventory ?? []).map((i) => (
              <li key={i.name}>
                {i.name} ×{i.qty}
                {i.equipped ? "（装备中）" : ""}
              </li>
            ))}
            {(meSnap.inventory ?? []).length === 0 && <li className="hint">背包是空的</li>}
          </ul>
        </div>
        {combat && combat.status === "active" && (
          <div className="lobby-card mp-combat">
            <strong>战斗中 · {myTurn ? "轮到你了" : `轮到${current?.name ?? "……"}`}</strong>
            <ul>
              {combat.units.map((u) => (
                <li key={u.id} className={`side-${u.side}${u.id === current?.id ? " now" : ""}${u.out ? " out" : ""}`}>
                  {u.name} {u.hp}/{u.max_hp}
                  {myTurn && u.side === "enemy" && !u.out && (
                    <button disabled={!!pending} onClick={() => fight("attack", u.id)}>
                      攻击
                    </button>
                  )}
                </li>
              ))}
            </ul>
            <div className="mp-fight">
              {(["defend", "dodge", "end_turn"] as const).map((k) => (
                <button key={k} disabled={!myTurn || !!pending} onClick={() => fight(k)}>
                  {{ defend: "防御", dodge: "闪避", end_turn: "结束回合" }[k]}
                </button>
              ))}
            </div>
          </div>
        )}
      </aside>
    </div>
  );
}

function GmChip({ gm }: { gm: GmStatus | null }) {
  if (!gm) return null;
  const label = gm.active === "local" ? `房主本机模型${gm.agent_model ? ` · ${gm.agent_model}` : ""}` : gm.active === "cloud" ? `云端模型 · ${gm.cloud_model}` : "GM 未就绪";
  return <span className={`mp-gm ${gm.ready ? "ok" : "off"}`}>AI GM：{label}</span>;
}

/** 房主的 GM 设置：模式与云端模型（Key 只发给服务端保存，之后不会再显示）。 */
function GmSettings({ gm, onSave, agentKey }: { gm: GmStatus | null; onSave: (mode: string, cloud: CloudGmConfig | null) => void; agentKey?: string }) {
  const [mode, setMode] = useState(gm?.mode ?? "auto");
  const [cloud, setCloud] = useState<CloudGmConfig>({ base_url: "", model: gm?.cloud_model ?? "", api_key: "", temperature: 0.8, context_length: 32768, timeout_secs: 120, max_tokens: 4096 });
  useEffect(() => {
    if (gm?.mode) setMode(gm.mode);
  }, [gm?.mode]);
  const set = <K extends keyof CloudGmConfig>(k: K, v: CloudGmConfig[K]) => setCloud((c) => ({ ...c, [k]: v }));
  return (
    <details className="lobby-card mp-gmset">
      <summary>GM 设置（只有房主能改）</summary>
      <label>
        模式
        <select value={mode} onChange={(e) => setMode(e.target.value)}>
          <option value="local">本机模型（GM Agent）</option>
          <option value="cloud">云端模型</option>
          <option value="auto">自动</option>
        </select>
      </label>
      {mode !== "local" && (
        <>
          <label>
            地址
            <input value={cloud.base_url} onChange={(e) => set("base_url", e.target.value)} placeholder="https://api.example.com/v1" />
          </label>
          <label>
            模型
            <input value={cloud.model} onChange={(e) => set("model", e.target.value)} />
          </label>
          <label>
            API Key
            <input type="password" value={cloud.api_key} onChange={(e) => set("api_key", e.target.value)} placeholder={gm?.cloud_has_key ? "已保存（留空沿用）" : ""} />
          </label>
          <label>
            温度
            <input type="number" step={0.1} min={0} max={2} value={cloud.temperature} onChange={(e) => set("temperature", Number(e.target.value))} />
          </label>
          <label>
            上下文长度
            <input type="number" value={cloud.context_length} onChange={(e) => set("context_length", Number(e.target.value))} />
          </label>
          <label>
            超时（秒）
            <input type="number" value={cloud.timeout_secs} onChange={(e) => set("timeout_secs", Number(e.target.value))} />
          </label>
          <label>
            最大输出
            <input type="number" value={cloud.max_tokens} onChange={(e) => set("max_tokens", Number(e.target.value))} />
          </label>
        </>
      )}
      <button className="primary" onClick={() => onSave(mode, mode !== "local" && cloud.base_url.trim() ? cloud : null)}>
        保存
      </button>
      {agentKey && <small className="hint">GM Agent 已在你的电脑上运行（密钥只在你手里）。</small>}
    </details>
  );
}
