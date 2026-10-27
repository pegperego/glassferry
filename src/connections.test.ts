import { describe, expect, it } from "vitest";
import type { Store } from "./api";
import {
  connectionErrors,
  emptyAppSource,
  groupByCategory,
  importAliases,
  mergeStores,
  newConnection,
  notImported,
  optionError,
  searchConnections,
} from "./connections";

function store(): Store {
  return {
    version: 1,
    categories: [
      { id: "dev", name: "Sviluppo", color: "#2e9d5b", protected: false },
      { id: "prod", name: "Produzione", color: "#d23c3c", protected: true },
    ],
    connections: [
      { ...newConnection("prod", { kind: "sshConfig", alias: "web-1" }), id: "a", name: "Web 10" },
      { ...newConnection("dev", { ...emptyAppSource(), host: "10.0.0.5", user: "deploy" }), id: "b", name: "Staging" },
      { ...newConnection("prod", { kind: "sshConfig", alias: "db" }), id: "c", name: "Web 9" },
    ],
  };
}

describe("searchConnections", () => {
  it("matches every term on name, target and category, sorted by category then name", () => {
    expect(searchConnections(store(), "").map((c) => c.id)).toEqual(["b", "c", "a"]);
    expect(searchConnections(store(), "prod web").map((c) => c.id)).toEqual(["c", "a"]);
    expect(searchConnections(store(), "deploy@10").map((c) => c.id)).toEqual(["b"]);
  });

  it("groups by category in store order", () => {
    const groups = groupByCategory(store(), searchConnections(store(), ""));
    expect(groups.map((g) => [g.category.id, g.items.length])).toEqual([
      ["dev", 1],
      ["prod", 2],
    ]);
  });
});

describe("import", () => {
  it("lists only aliases not imported and imports them with a category", () => {
    const hosts = [
      { alias: "db", source: "~/.ssh/config" },
      { alias: "new", source: "~/.ssh/config" },
    ];
    expect(notImported(hosts, store()).map((h) => h.alias)).toEqual(["new"]);
    const imported = importAliases(store(), ["new"], "dev").connections[3];
    expect(imported).toMatchObject({ name: "new", categoryId: "dev", source: { kind: "sshConfig", alias: "new" } });
  });

  it("merges an exported file by id", () => {
    const incoming = store();
    incoming.categories.push({ id: "qa", name: "QA", color: "#000", protected: false });
    incoming.connections.push({ ...newConnection("qa"), id: "z", name: "QA box" });
    const merged = mergeStores(store(), incoming);
    expect([merged.added, merged.skipped]).toEqual([1, 3]);
    expect(merged.store.categories.map((c) => c.id)).toEqual(["dev", "prod", "qa"]);
  });
});

describe("validation", () => {
  it("checks required fields, ports and options", () => {
    const s = store();
    const c = newConnection("dev", { ...emptyAppSource(), port: 70000, extraOptions: ["Include x", "Compression yes"] });
    expect(Object.keys(connectionErrors(c, s)).sort()).toEqual(["extraOptions", "host", "name", "port"]);
    expect(connectionErrors({ ...s.connections[1], name: "Web 9" }, s).name).toMatch(/Esiste già/);
    expect(connectionErrors(s.connections[1], s)).toEqual({});
  });

  it("accepts Keyword value and Keyword=value", () => {
    expect(optionError("ServerAliveInterval 30")).toBeNull();
    expect(optionError("ServerAliveInterval=30")).toBeNull();
    expect(optionError("Compression")).not.toBeNull();
    expect(optionError("match all")).toMatch(/non è ammessa/);
  });
});
