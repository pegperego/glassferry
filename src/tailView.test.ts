import { describe, expect, it } from "vitest";
import { append, compileFilter, emptyBuffer, filterLines, highlight } from "./tailView";

describe("append", () => {
  it("keeps ids increasing and drops the oldest lines", () => {
    let buffer = append(emptyBuffer(), ["a", "b", "c"], false, 4);
    buffer = append(buffer, ["— troncato —"], true, 4);
    buffer = append(buffer, ["d", "e"], false, 4);
    expect(buffer.lines.map((l) => l.text)).toEqual(["c", "— troncato —", "d", "e"]);
    expect(buffer.lines.map((l) => l.id)).toEqual([3, 4, 5, 6]);
    expect(buffer.lines[1].notice).toBe(true);
    expect(buffer.dropped).toBe(2);
  });

  it("returns the same buffer when nothing is added", () => {
    const buffer = emptyBuffer();
    expect(append(buffer, [])).toBe(buffer);
  });
});

describe("compileFilter", () => {
  it("compiles regular expressions, case-insensitive by default", () => {
    expect(compileFilter("", false)).toEqual({ regex: null, error: null });
    expect(compileFilter("err(or)?", false).regex?.test("ERROR")).toBe(true);
    expect(compileFilter("error", true).regex?.test("ERROR")).toBe(false);
    const bad = compileFilter("(", false);
    expect(bad.regex).toBeNull();
    expect(bad.error).toBeTruthy();
  });
});

describe("filterLines", () => {
  it("keeps matching lines and every notice", () => {
    const buffer = append(append(emptyBuffer(), ["GET /a 200", "GET /b 500"]), ["— ruotato —"], true);
    const { regex } = compileFilter(" 5\\d\\d$", false);
    expect(filterLines(buffer.lines, regex).map((l) => l.text)).toEqual(["GET /b 500", "— ruotato —"]);
    expect(filterLines(buffer.lines, null)).toBe(buffer.lines);
  });

  it("is not fooled by the global flag between calls", () => {
    const buffer = append(emptyBuffer(), ["error 1", "error 2", "error 3"]);
    const { regex } = compileFilter("error", false);
    expect(filterLines(buffer.lines, regex)).toHaveLength(3);
    expect(filterLines(buffer.lines, regex)).toHaveLength(3);
  });
});

describe("highlight", () => {
  it("splits text into matches", () => {
    const { regex } = compileFilter("o", false);
    expect(highlight("foo bar", regex)).toEqual([
      { text: "f", match: false },
      { text: "o", match: true },
      { text: "o", match: true },
      { text: " bar", match: false },
    ]);
    expect(highlight("abc", null)).toEqual([{ text: "abc", match: false }]);
  });

  it("survives empty matches", () => {
    const { regex } = compileFilter("x*", false);
    expect(highlight("axb", regex)).toEqual([
      { text: "a", match: false },
      { text: "x", match: true },
      { text: "b", match: false },
    ]);
    expect(highlight("", regex)).toEqual([{ text: "", match: false }]);
  });
});
