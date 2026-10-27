// Pure logic on the connection store: search, import, merge, validation.
// Validation mirrors src-tauri/src/connections.rs (the backend checks again).

import type { Category, Connection, HostEntry, Source, Store } from "./api";

/** Keywords that would break the generated ssh_config. */
const FORBIDDEN_KEYWORDS = ["host", "match", "include"];

export function newId(): string {
  return crypto.randomUUID().replace(/-/g, "").slice(0, 12);
}

export function emptyAppSource(): Extract<Source, { kind: "app" }> {
  return { kind: "app", host: "", user: null, port: null, identityFile: null, proxyJump: null, extraOptions: [] };
}

export function newConnection(categoryId: string, source: Source = emptyAppSource()): Connection {
  return { id: newId(), name: "", categoryId, source, remoteDir: null, localDir: null, savePassword: false };
}

/** `alias` or `user@host:port`, shown under the name. */
export function describeSource(source: Source): string {
  if (source.kind === "sshConfig") return source.alias;
  const user = source.user ? `${source.user}@` : "";
  const port = source.port ? `:${source.port}` : "";
  return `${user}${source.host}${port}`;
}

export function categoryOf(store: Store, connection: Connection): Category | undefined {
  return store.categories.find((c) => c.id === connection.categoryId);
}

/** Connections whose name, alias, host, user or category contain every term. */
export function searchConnections(store: Store, query: string): Connection[] {
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  const matches = store.connections.filter((c) => {
    const haystack = [c.name, describeSource(c.source), categoryOf(store, c)?.name ?? ""].join(" ").toLowerCase();
    return terms.every((t) => haystack.includes(t));
  });
  return sortByCategory(store, matches);
}

/** Category order of the store, then name. */
export function sortByCategory(store: Store, connections: Connection[]): Connection[] {
  const order = new Map(store.categories.map((c, i) => [c.id, i]));
  return [...connections].sort(
    (a, b) =>
      (order.get(a.categoryId) ?? Infinity) - (order.get(b.categoryId) ?? Infinity) ||
      a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" }),
  );
}

export function groupByCategory(store: Store, connections: Connection[]): { category: Category; items: Connection[] }[] {
  return store.categories
    .map((category) => ({ category, items: connections.filter((c) => c.categoryId === category.id) }))
    .filter((g) => g.items.length > 0);
}

/** Aliases of ~/.ssh/config not imported yet. */
export function notImported(hosts: HostEntry[], store: Store): HostEntry[] {
  const imported = new Set(
    store.connections.flatMap((c) => (c.source.kind === "sshConfig" ? [c.source.alias] : [])),
  );
  return hosts.filter((h) => !imported.has(h.alias));
}

export function importAliases(store: Store, aliases: string[], categoryId: string): Store {
  const added = aliases.map((alias) => ({
    ...newConnection(categoryId, { kind: "sshConfig", alias }),
    name: alias,
  }));
  return { ...store, connections: [...store.connections, ...added] };
}

/** Adds categories and connections of an exported file that are not present (by id). */
export function mergeStores(store: Store, incoming: Store): { store: Store; added: number; skipped: number } {
  const categoryIds = new Set(store.categories.map((c) => c.id));
  const connectionIds = new Set(store.connections.map((c) => c.id));
  const newConnections = incoming.connections.filter((c) => !connectionIds.has(c.id));
  return {
    store: {
      ...store,
      categories: [...store.categories, ...incoming.categories.filter((c) => !categoryIds.has(c.id))],
      connections: [...store.connections, ...newConnections],
    },
    added: newConnections.length,
    skipped: incoming.connections.length - newConnections.length,
  };
}

export function upsertConnection(store: Store, connection: Connection): Store {
  const exists = store.connections.some((c) => c.id === connection.id);
  return {
    ...store,
    connections: exists
      ? store.connections.map((c) => (c.id === connection.id ? connection : c))
      : [...store.connections, connection],
  };
}

export function removeConnection(store: Store, id: string): Store {
  return { ...store, connections: store.connections.filter((c) => c.id !== id) };
}

export function duplicateConnection(connection: Connection): Connection {
  return { ...connection, id: newId(), name: `${connection.name} (copia)`, savePassword: false };
}

/** Error of one `Keyword value` line, or null. */
export function optionError(line: string): string | null {
  const match = /^([A-Za-z0-9]+)(?:\s*=\s*|\s+)(\S.*)$/.exec(line.trim());
  if (!match) return `"${line.trim()}": formato Chiave valore`;
  if (FORBIDDEN_KEYWORDS.includes(match[1].toLowerCase())) return `${match[1]} non è ammessa`;
  return null;
}

/** One option per line, blank lines ignored. */
export function parseOptions(text: string): string[] {
  return text
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);
}

const WORD = /^[^\s"]+$/;

/** Field errors of a connection being edited (empty when valid). */
export function connectionErrors(connection: Connection, store: Store): Partial<Record<string, string>> {
  const errors: Partial<Record<string, string>> = {};
  if (!connection.name.trim()) errors.name = "Il nome è obbligatorio";
  else if (store.connections.some((c) => c.id !== connection.id && c.name.trim() === connection.name.trim()))
    errors.name = "Esiste già una connessione con questo nome";
  if (!store.categories.some((c) => c.id === connection.categoryId)) errors.categoryId = "Scegli una categoria";
  const source = connection.source;
  if (source.kind === "sshConfig") {
    if (!WORD.test(source.alias)) errors.alias = "Alias non valido";
    return errors;
  }
  if (!WORD.test(source.host)) errors.host = source.host ? "Host non valido" : "L'host è obbligatorio";
  if (source.user !== null && !WORD.test(source.user)) errors.user = "Utente non valido";
  if (source.port !== null && (!Number.isInteger(source.port) || source.port < 1 || source.port > 65535))
    errors.port = "Porta tra 1 e 65535";
  if (source.identityFile !== null && (!source.identityFile.trim() || source.identityFile.includes('"')))
    errors.identityFile = "Percorso non valido";
  if (source.proxyJump !== null && !WORD.test(source.proxyJump)) errors.proxyJump = "ProxyJump non valido";
  const optionErrors = source.extraOptions.map(optionError).filter(Boolean);
  if (optionErrors.length > 0) errors.extraOptions = optionErrors.join("; ");
  return errors;
}

export function addCategory(store: Store, name: string, color: string): Store {
  return { ...store, categories: [...store.categories, { id: newId(), name, color, protected: false }] };
}

export function updateCategory(store: Store, category: Category): Store {
  return { ...store, categories: store.categories.map((c) => (c.id === category.id ? category : c)) };
}

/** A category can be deleted only when no connection uses it. */
export function categoryInUse(store: Store, id: string): number {
  return store.connections.filter((c) => c.categoryId === id).length;
}

export function removeCategory(store: Store, id: string): Store {
  return { ...store, categories: store.categories.filter((c) => c.id !== id) };
}
