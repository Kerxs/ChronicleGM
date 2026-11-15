import { describe, expect, it } from "vitest";
import { splitForSpeech } from "./speechChunks";

describe("朗读分段", () => {
  it("短讲述只有一段", () => {
    expect(splitForSpeech("雨夜，你推开酒馆的门。")).toEqual(["雨夜，你推开酒馆的门。"]);
  });
  it("长讲述：第一段短，后面每段不超过上限，内容不丢不乱", () => {
    const para = "雾从海面涌上来，码头上挤满了焦急的镇民。北边的灯塔一片漆黑，三十年来从没熄过的火，今晚熄了。";
    const text = `欢迎来到《灯塔守夜》。\n\n${para.repeat(4)}\n\n${para.repeat(3)}\n你打算怎么做？`;
    const chunks = splitForSpeech(text);
    expect(chunks.length).toBeGreaterThan(3);
    expect(chunks[0].length).toBeLessThanOrEqual(80);
    for (const c of chunks) expect(c.length).toBeLessThanOrEqual(200 + para.length);
    expect(chunks.join("")).toBe(text.replace(/\s+/g, ""));
    expect(chunks[chunks.length - 1].endsWith("你打算怎么做？")).toBe(true);
  });
  it("没有标点的长句单独成段", () => {
    const long = "啊".repeat(500);
    const chunks = splitForSpeech(long);
    expect(chunks.join("")).toBe(long);
  });
  it("引号跟着句末标点", () => {
    const chunks = splitForSpeech("她说：「快走！」然后转身。", 5, 10);
    expect(chunks[0]).toBe("她说：「快走！」");
  });
});
