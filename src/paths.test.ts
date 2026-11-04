import { describe, expect, it } from "vitest";
import { isAbsolutePath, joinPath, parentPath, pathSegments } from "./paths";

describe("POSIX paths", () => {
  it("joins, goes up and splits", () => {
    expect(joinPath("/srv", "www")).toBe("/srv/www");
    expect(joinPath("/", "C:")).toBe("/C:");
    expect(parentPath("/srv/www/")).toBe("/srv");
    expect(parentPath("/srv")).toBe("/");
    expect(parentPath("/C:")).toBe("/");
    expect(pathSegments("/srv/www").map((s) => s.path)).toEqual(["/", "/srv", "/srv/www"]);
    expect(isAbsolutePath("C:/x")).toBe(false);
  });
});

describe("Windows local paths (drives)", () => {
  it("treats / as the list of drives", () => {
    expect(joinPath("/", "C:", true)).toBe("C:/");
    expect(joinPath("C:/", "Users", true)).toBe("C:/Users");
    expect(joinPath("C:/Users", "Roberto", true)).toBe("C:/Users/Roberto");
    expect(parentPath("C:/Users/Roberto", true)).toBe("C:/Users");
    expect(parentPath("C:/Users", true)).toBe("C:/");
    expect(parentPath("C:/", true)).toBe("/");
    expect(parentPath("D:", true)).toBe("/");
  });

  it("builds breadcrumbs with the drive", () => {
    expect(pathSegments("C:/Users/Roberto", true)).toEqual([
      { name: "/", path: "/" },
      { name: "C:", path: "C:/" },
      { name: "Users", path: "C:/Users" },
      { name: "Roberto", path: "C:/Users/Roberto" },
    ]);
    expect(pathSegments("/", true)).toEqual([{ name: "/", path: "/" }]);
  });

  it("accepts drive paths as absolute", () => {
    expect(isAbsolutePath("C:/x", true)).toBe(true);
    expect(isAbsolutePath("x/y", true)).toBe(false);
  });
});
