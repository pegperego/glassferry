import { describe, expect, it } from "vitest";
import type { Store } from "./api";
import {
  addBookmark,
  bookmarksOf,
  confirmationMatches,
  connectionErrors,
  emptyAppSource,
  groupByCategory,
  importAliases,
  mergeStores,
  newConnection,
  notImported,
  optionError,
  protectionOf,
  removeBookmark,
  removeConnection,
  searchBookmarks,
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
    bookmarks: [],
  };
}

describe("bookmarks", () => {
  it("adds once per folder, named after it, and removes", () => {
    let s = addBookmark(store(), "a", "/var/www/site/");
    s = addBookmark(s, "a", "/var/www/site");
    s = addBookmark(s, null, "/");
    expect(s.bookmarks.map((b) => [b.connectionId, b.name, b.path])).toEqual([
      ["a", "site", "/var/www/site"],
      [null, "/", "/"],
    ]);
    expect(bookmarksOf(s, "a")).toHaveLength(1);
    expect(bookmarksOf(s, "b")).toHaveLength(0);
    expect(removeBookmark(s, s.bookmarks[0].id).bookmarks).toHaveLength(1);
  });

  it("go away with their connection", () => {
    const s = addBookmark(addBookmark(store(), "a", "/srv"), "b", "/srv");
    expect(removeConnection(s, "a").bookmarks.map((b) => b.connectionId)).toEqual(["b"]);
  });

  it("are found by name, path or connection", () => {
    const s = addBookmark(addBookmark(store(), "a", "/var/www/shop"), null, "/Users/me/www");
    expect(searchBookmarks(s, "shop").map((r) => r.connection.id)).toEqual(["a"]);
    expect(searchBookmarks(s, "web 10")).toHaveLength(1);
    // Local bookmarks are not connections.
    expect(searchBookmarks(s, "users")).toHaveLength(0);
    expect(searchBookmarks(s, "")).toHaveLength(1);
  });

  it("merge from an import only for known connections", () => {
    const incoming = { ...store(), connections: [], bookmarks: [] } as Store;
    incoming.bookmarks = [
      { id: "x1", name: "logs", connectionId: "a", path: "/var/log" },
      { id: "x2", name: "gone", connectionId: "zzz", path: "/tmp" },
    ];
    const mine = addBookmark(store(), "a", "/var/log");
    const merged = mergeStores(mine, incoming).store;
    expect(merged.bookmarks).toHaveLength(1);
    expect(mergeStores(store(), incoming).store.bookmarks.map((b) => b.id)).toEqual(["x1"]);
  });
});

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

describe("protection", () => {
  it("requires the exact connection name only on protected categories", () => {
    const s = store();
    const prod = protectionOf(s, s.connections[0]);
    const dev = protectionOf(s, s.connections[1]);
    expect([prod.protected, dev.protected]).toEqual([true, false]);
    expect(confirmationMatches(prod, "web 10")).toBe(false);
    expect(confirmationMatches(prod, " Web 10 ")).toBe(true);
    expect(confirmationMatches(dev, "")).toBe(true);
    expect(confirmationMatches(null, "")).toBe(true);
  });
});
