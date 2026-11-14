import { describe, expect, it } from "vitest";
import { catalogFor, getFlag, setFlag, unknownArgs } from "./launchFlags";
import { cache_ram_label } from "./launchSummary";
import { mentioned } from "./mentions";

// 你的 启动.bat 拆出来的参数。
const BAT = ["-m", "D:\\llama\\models\\Ornith.gguf", "--mmproj", "D:\\llama\\models\\mmproj.gguf", "--no-mmproj-offload", "-ngl", "auto", "-c", "32768", "-ctk", "q8_0", "--fit", "on", "-lm", "none", "--port", "8080"];
const llama = catalogFor("D:\\llama\\llama-server.exe", "llm");
const spec = (k: string) => llama.find((s) => s.key === k)!;

describe("启动参数编辑", () => {
  it("按程序名选目录", () => {
    expect(llama.length).toBeGreaterThan(10);
    expect(catalogFor("D:\\b\\breeze-server.exe", "tts").some((s) => s.positional)).toBe(true);
    expect(catalogFor("D:\\x\\koboldcpp.exe", "llm")).toEqual([]);
  });
  it("读取：值、开关、没写", () => {
    expect(getFlag(BAT, spec("ctx"))).toBe("32768");
    expect(getFlag(BAT, spec("mmproj_cpu"))).toBe("on");
    expect(getFlag(BAT, spec("cache_ram"))).toBeNull();
    expect(getFlag(BAT, spec("load"))).toBe("none");
    expect(getFlag(["-cram", "-1"], spec("cache_ram"))).toBe("-1");
  });
  it("写入：改值保持位置，新参数追加，删除连同值一起删", () => {
    const a = setFlag(BAT, spec("ctx"), "16384");
    expect(a.indexOf("-c")).toBe(BAT.indexOf("-c"));
    expect(a[a.indexOf("-c") + 1]).toBe("16384");
    expect(BAT[BAT.indexOf("-c") + 1]).toBe("32768"); // 不改原数组
    const b = setFlag(BAT, spec("cache_ram"), "2048");
    expect(b.slice(-2)).toEqual(["--cache-ram", "2048"]);
    const c = setFlag(BAT, spec("mmproj"), null);
    expect(c).not.toContain("--mmproj");
    expect(c).not.toContain("D:\\llama\\models\\mmproj.gguf");
    expect(c.length).toBe(BAT.length - 2);
    const d = setFlag(BAT, spec("mmproj_cpu"), null);
    expect(d.length).toBe(BAT.length - 1);
    expect(setFlag(d, spec("mmproj_cpu"), "on").at(-1)).toBe("--no-mmproj-offload");
  });
  it("别名也能认出来，并原地修改", () => {
    const a = setFlag(["--ctx-size", "4096"], spec("ctx"), "8192");
    expect(a).toEqual(["--ctx-size", "8192"]);
  });
  it("breeze 的模型文件是第一个位置参数", () => {
    const br = catalogFor("breeze-server.exe", "tts");
    const model = br.find((s) => s.key === "model")!;
    expect(getFlag(["m.gguf", "--port", "8137"], model)).toBe("m.gguf");
    expect(setFlag(["m.gguf", "--port", "8137"], model, "n.gguf")).toEqual(["n.gguf", "--port", "8137"]);
    expect(setFlag(["--port", "8137"], model, "n.gguf")).toEqual(["n.gguf", "--port", "8137"]);
    expect(getFlag(["--cpu"], model)).toBeNull();
  });
  it("目录外的参数原样列出", () => {
    expect(unknownArgs([...BAT, "--jinja", "--slots"], llama)).toEqual(["--jinja", "--slots"]);
  });
  it("列表摘要", () => {
    expect(cache_ram_label(BAT)).toBe("提示词缓存：默认 8 GB");
    expect(cache_ram_label(["--cache-ram", "2048"])).toBe("提示词缓存：最多 2 GB");
    expect(cache_ram_label(["-cram", "0"])).toBe("提示词缓存：关闭");
    expect(cache_ram_label(["-cram", "-1"])).toBe("提示词缓存：不限制");
  });
});

describe("叙述里提到的配图设定", () => {
  const n = (title: string, aliases: string[] = []) => ({ node_id: title, title, aliases, kind: "site", count: 1 });
  it("按出现顺序，长名字覆盖短名字，忽略单字别名", () => {
    const nodes = [n("雾港"), n("雾港税关", ["税关"]), n("灯塔岬", ["岬"]), n("黑潮湾")];
    const r = mentioned("你们离开雾港税关，沿着海岸走向灯塔岬。", nodes);
    expect(r.map((x) => x.title)).toEqual(["雾港税关", "灯塔岬"]);
    expect(mentioned("税关的灯还亮着", nodes).map((x) => x.title)).toEqual(["雾港税关"]);
    expect(mentioned("海边一个岬角", nodes)).toEqual([]);
  });
});

import { promptFor } from "./imagePrompt";
describe("生图描述", () => {
  it("用标题、类型、摘要与玩家描述，不含主持人隐藏内容", () => {
    const n = { title: "老板娘梅", kind: "character" as const, summary: "锚链酒馆的老板娘", player_text: "眼角有一道旧疤", gm_text: "她其实是走私头目" };
    const p = promptFor(n, "水彩画风格");
    expect(p).toContain("人物「老板娘梅」的人物肖像");
    expect(p).toContain("锚链酒馆的老板娘");
    expect(p).toContain("眼角有一道旧疤");
    expect(p).toContain("水彩画风格");
    expect(p).not.toContain("走私");
    expect(promptFor({ ...n, kind: "city", title: "雾港", summary: "", player_text: "" }, "")).toMatch(/^城市「雾港」的场景。/);
  });
});
