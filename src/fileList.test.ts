import { describe, expect, it } from "vitest";
import type { FileEntry } from "./api";
import { clickSelect, emptySelection, moveCursor, pruneSelection, visibleEntries } from "./fileList";

const entry = (name: string, kind: FileEntry["kind"], size = 0, modified = 0): FileEntry => ({
  name,
  kind,
  targetIsDir: false,
  size,
  modified,
  permissions: 0o644,
});

describe("visibleEntries", () => {
  const entries = [
    entry("b.txt", "file", 10, 3),
    entry("file10", "file", 30, 1),
    entry("file2", "file", 20, 2),
    entry("zdir", "dir"),
    entry(".hidden", "file"),
    { ...entry("adir-link", "symlink"), targetIsDir: true },
  ];

  it("puts directories first and sorts names naturally", () => {
    const names = visibleEntries(entries, { key: "name", ascending: true }, false).map((e) => e.name);
    expect(names).toEqual(["adir-link", "zdir", "b.txt", "file2", "file10"]);
  });

  it("keeps directories first when descending by size", () => {
    const names = visibleEntries(entries, { key: "size", ascending: false }, true).map((e) => e.name);
    expect(names).toEqual(["zdir", "adir-link", "file10", "file2", "b.txt", ".hidden"]);
  });
});

describe("selection", () => {
  const names = ["a", "b", "c", "d"];

  it("extends a range from the anchor with shift-click", () => {
    let sel = clickSelect(emptySelection, names, "b", { toggle: false, extend: false });
    sel = clickSelect(sel, names, "d", { toggle: false, extend: true });
    expect([...sel.selected]).toEqual(["b", "c", "d"]);
    sel = clickSelect(sel, names, "a", { toggle: false, extend: true });
    expect([...sel.selected]).toEqual(["a", "b"]);
  });

  it("toggles single items with cmd-click", () => {
    let sel = clickSelect(emptySelection, names, "a", { toggle: false, extend: false });
    sel = clickSelect(sel, names, "c", { toggle: true, extend: false });
    sel = clickSelect(sel, names, "a", { toggle: true, extend: false });
    expect([...sel.selected]).toEqual(["c"]);
  });

  it("moves the cursor within bounds and extends with shift", () => {
    let sel = moveCursor(emptySelection, names, 1, false);
    expect(sel.cursor).toBe("a");
    sel = moveCursor(sel, names, 10, true);
    expect(sel.cursor).toBe("d");
    expect(sel.selected.size).toBe(4);
    sel = moveCursor(sel, names, -1, false);
    expect([...sel.selected]).toEqual(["c"]);
  });

  it("prunes names that disappeared", () => {
    const sel = clickSelect(emptySelection, names, "b", { toggle: false, extend: false });
    expect(pruneSelection(sel, ["a", "c"])).toEqual({ selected: new Set(), cursor: null, anchor: null });
  });
});
