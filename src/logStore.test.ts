import { describe, expect, it } from "vitest";
import type { LogEntry } from "./api";
import { clampCapacity, filterEntries, emptyFilter, mergeBatch } from "./logStore";

const entry = (id: number, patch: Partial<LogEntry> = {}): LogEntry => ({
  id,
  time: id,
  host: "web",
  channel: "sftp",
  op: "readdir",
  detail: `/dir${id}`,
  status: "ok",
  durationMs: 1,
  extra: null,
  error: null,
  output: null,
  equivalent: null,
  ...patch,
});

describe("mergeBatch", () => {
  it("appends new entries and updates pending ones in place", () => {
    const start = mergeBatch([], [entry(1, { status: "pending" }), entry(2)], 10);
    const next = mergeBatch(start, [entry(1, { status: "error" }), entry(3)], 10);
    expect(next.map((e) => [e.id, e.status])).toEqual([
      [1, "error"],
      [2, "ok"],
      [3, "ok"],
    ]);
  });

  it("trims to capacity and ignores updates of evicted entries", () => {
    const full = mergeBatch([], [entry(1, { status: "pending" }), entry(2), entry(3)], 2);
    expect(full.map((e) => e.id)).toEqual([2, 3]);
    const next = mergeBatch(full, [entry(1, { status: "ok" })], 2);
    expect(next.map((e) => e.id)).toEqual([2, 3]);
  });
});

describe("filterEntries", () => {
  const entries = [
    entry(1),
    entry(2, { status: "error", error: "Permission denied", host: "db" }),
    entry(3, { channel: "ssh", op: "connect" }),
  ];

  it("combines errors-only, host, channel and text filters", () => {
    expect(filterEntries(entries, { ...emptyFilter, errorsOnly: true }).map((e) => e.id)).toEqual([2]);
    expect(filterEntries(entries, { ...emptyFilter, host: "web", channel: "ssh" }).map((e) => e.id)).toEqual([3]);
    expect(filterEntries(entries, { ...emptyFilter, text: "permission" }).map((e) => e.id)).toEqual([2]);
  });
});

describe("clampCapacity", () => {
  it("keeps the value within bounds", () => {
    expect(clampCapacity(5)).toBe(100);
    expect(clampCapacity(2500.4)).toBe(2500);
    expect(clampCapacity(Number.NaN)).toBe(1000);
  });
});
