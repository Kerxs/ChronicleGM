import { describe, expect, it } from "vitest";
import { parseFaces } from "./DiceAsk";

describe("parseFaces", () => {
  it("接受空格、逗号、顿号分隔的点数", () => {
    expect(parseFaces("14", 1, 20)).toEqual([14]);
    expect(parseFaces("3 5，6、1", 4, 6)).toEqual([3, 5, 6, 1]);
  });
  it("颗数不对、超出骰面、不是整数时拒绝", () => {
    expect(parseFaces("", 1, 20)).toBeNull();
    expect(parseFaces("21", 1, 20)).toBeNull();
    expect(parseFaces("0", 1, 20)).toBeNull();
    expect(parseFaces("2.5", 1, 20)).toBeNull();
    expect(parseFaces("3 4", 3, 6)).toBeNull();
  });
});
