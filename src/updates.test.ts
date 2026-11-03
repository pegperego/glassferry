import { describe, expect, it } from "vitest";
import type { EditInfo, JobInfo } from "./api";
import { percentOf, restartImpact, shouldOffer } from "./updates";

const job = (id: number, state: JobInfo["state"]) => ({ id, state }) as JobInfo;
const edit = (id: number, state: EditInfo["state"]) => ({ id, state }) as EditInfo;

describe("shouldOffer", () => {
  it("hides the skipped version from automatic checks only", () => {
    expect(shouldOffer("0.4.0", "0.4.0", false)).toBe(false);
    expect(shouldOffer("0.4.0", "0.4.0", true)).toBe(true);
    expect(shouldOffer("0.4.1", "0.4.0", false)).toBe(true);
    expect(shouldOffer("0.4.0", null, false)).toBe(true);
  });
});

describe("restartImpact", () => {
  it("lists running transfers and files with changes not uploaded", () => {
    const impact = restartImpact(
      [job(1, "running"), job(2, "done"), job(3, "waitingConflict"), job(4, "failed"), job(5, "scanning")],
      [edit(1, "synced"), edit(2, "detached"), edit(3, "uploading")],
    );
    expect(impact.transfers.map((j) => j.id)).toEqual([1, 3, 5]);
    expect(impact.edits.map((e) => e.id)).toEqual([2, 3]);
  });
});

describe("percentOf", () => {
  it("is null while the size is unknown and never above 100", () => {
    expect(percentOf(10, null)).toBeNull();
    expect(percentOf(10, 0)).toBeNull();
    expect(percentOf(1, 3)).toBe(33);
    expect(percentOf(5, 4)).toBe(100);
  });
});
