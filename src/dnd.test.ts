import { describe, expect, it } from "vitest";
import { groupByParent } from "./dnd";

describe("groupByParent", () => {
  it("groups Finder paths by parent directory", () => {
    const groups = groupByParent(["/Users/a/x.txt", "/Users/a/dir/", "/Users/b/y", "/z"]);
    expect([...groups]).toEqual([
      ["/Users/a", ["x.txt", "dir"]],
      ["/Users/b", ["y"]],
      ["/", ["z"]],
    ]);
  });
});
