import { afterEach, describe, expect, it } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { api, formatTime } from "./api";

// 注意：这些测试使用 mockIPC 模拟后端，只验证前端调用约定，不代表真实后端行为。
afterEach(() => clearMocks());

describe("api (mock IPC)", () => {
  it("把参数原样传给后端命令", async () => {
    const calls: Array<[string, unknown]> = [];
    mockIPC((cmd, args) => {
      calls.push([cmd, args]);
      if (cmd === "create_world") return { id: "W1", name: (args as { name: string }).name };
      return null;
    });
    const w = await api.createWorld("艾尔大陆");
    expect(w.name).toBe("艾尔大陆");
    expect(calls[0][0]).toBe("create_world");
    expect(calls[0][1]).toMatchObject({ name: "艾尔大陆" });
  });

  it("世界模拟命令的参数名与后端一致", async () => {
    const calls: Array<[string, unknown]> = [];
    mockIPC((cmd, args) => {
      calls.push([cmd, args]);
      return null;
    });
    await api.worldSimOverview("W", "C", true);
    await api.planTravel("W", "C", "P");
    await api.travelTo("W", "C", "P");
    await api.addMapMarker("W", "C", "P", "宝藏", "");
    await api.setWeatherToday("W", "C", "暴雨");
    expect(calls.map((c) => c[0])).toEqual(["world_sim_overview", "plan_travel", "travel_to", "add_map_marker", "set_weather_today"]);
    expect(calls[0][1]).toMatchObject({ worldId: "W", campaignId: "C", gmView: true });
    expect(calls[2][1]).toMatchObject({ to: "P" });
    expect(calls[3][1]).toMatchObject({ place: "P", label: "宝藏" });
  });

  it("角色与战斗命令的参数名与后端一致", async () => {
    const calls: Array<[string, unknown]> = [];
    mockIPC((cmd, args) => {
      calls.push([cmd, args]);
      return null;
    });
    await api.rpgOverview("W", "C");
    await api.rpgAction("W", "C", "buy", "M", "I", 2, "");
    await api.combatStart("W", "C", [{ name: "狼", npc: null, level: 1, count: 2, loot: [], boss: false }], [], "遭遇");
    await api.combatAct("W", "C", { kind: "attack", target: "e1", item: "", x: null, y: null, custom: "" });
    expect(calls.map((c) => c[0])).toEqual(["rpg_overview", "rpg_action", "combat_start", "combat_act"]);
    expect(calls[1][1]).toMatchObject({ input: { kind: "buy", target: "M", item: "I", qty: 2 } });
    expect(calls[2][1]).toMatchObject({ allies: [], reason: "遭遇" });
    expect(calls[3][1]).toMatchObject({ action: { kind: "attack", target: "e1" } });
  });

  it("控制台与导演命令的参数名与后端一致", async () => {
    const calls: Array<[string, unknown]> = [];
    mockIPC((cmd, args) => {
      calls.push([cmd, args]);
      return null;
    });
    await api.gmConsole("W", "C");
    await api.consoleApply("W", "C", [{ type: "set_flag", name: "x", value: true }], "原因");
    await api.directorSetStatus("W", "C", "n1", "n1-1", "adopted");
    await api.directorAnalyze("W", "C");
    expect(calls.map((c) => c[0])).toEqual(["gm_console", "console_apply", "director_set_status", "director_analyze"]);
    expect(calls[1][1]).toMatchObject({ reason: "原因", effects: [{ type: "set_flag" }] });
    expect(calls[2][1]).toMatchObject({ noteId: "n1", suggestionId: "n1-1", status: "adopted" });
  });

  it("后端错误字符串转换为 Error", async () => {
    mockIPC(() => {
      throw "世界名称不能为空";
    });
    await expect(api.createWorld(" ")).rejects.toThrow("世界名称不能为空");
  });
});

describe("formatTime", () => {
  it("空值显示占位符", () => {
    expect(formatTime(null)).toBe("—");
  });
  it("格式化为本地时间", () => {
    const ms = new Date(2026, 10, 12, 9, 5).getTime();
    expect(formatTime(ms)).toBe("2026-11-12 09:05");
  });
});
