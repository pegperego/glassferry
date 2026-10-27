import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import type { Store } from "../api";
import { categoryOf, describeSource, groupByCategory, searchConnections } from "../connections";

interface Props {
  store: Store;
  onConnect: (connectionId: string) => void;
  onManage: () => void;
  onClose: () => void;
}

/** "Nuovo collegamento" (⌘K): search a connection, Enter connects. */
export function QuickConnect({ store, onConnect, onManage, onClose }: Props) {
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);
  const matches = useMemo(() => searchConnections(store, query), [store, query]);
  const groups = useMemo(() => groupByCategory(store, matches), [store, matches]);
  // Display order (grouped) drives the keyboard cursor.
  const ordered = groups.flatMap((g) => g.items);
  const listRef = useRef<HTMLUListElement>(null);

  useEffect(() => setCursor(0), [query]);
  useEffect(() => {
    listRef.current?.querySelector(".cursor")?.scrollIntoView({ block: "nearest" });
  }, [cursor]);

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") onClose();
    else if (e.key === "ArrowDown") setCursor((c) => Math.min(c + 1, ordered.length - 1));
    else if (e.key === "ArrowUp") setCursor((c) => Math.max(c - 1, 0));
    else if (e.key === "Enter" && ordered[cursor]) onConnect(ordered[cursor].id);
    else return;
    e.preventDefault();
  };

  return (
    <div className="modal-backdrop top" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal palette" onKeyDown={onKey}>
        <input
          className="palette-input"
          autoFocus
          placeholder="Cerca per nome, host, alias o categoria…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        {store.connections.length === 0 ? (
          <p className="muted palette-empty">
            Nessuna connessione. Importale da ~/.ssh/config o creane una nuova in "Gestisci connessioni".
          </p>
        ) : ordered.length === 0 ? (
          <p className="muted palette-empty">Nessuna connessione corrisponde.</p>
        ) : (
          <ul className="palette-list" ref={listRef}>
            {groups.map((group) => (
              <li key={group.category.id}>
                <div className="palette-group">
                  <span className="category-dot" style={{ background: group.category.color }} />
                  {group.category.name}
                  {group.category.protected && <span className="muted"> · protetta</span>}
                </div>
                <ul>
                  {group.items.map((connection) => {
                    const index = ordered.indexOf(connection);
                    return (
                      <li
                        key={connection.id}
                        className={index === cursor ? "palette-item cursor" : "palette-item"}
                        onMouseMove={() => setCursor(index)}
                        onClick={() => onConnect(connection.id)}
                      >
                        <span
                          className="category-bar"
                          style={{ background: categoryOf(store, connection)?.color }}
                        />
                        <span className="palette-name">{connection.name}</span>
                        {describeSource(connection.source) !== connection.name && (
                          <span className="muted palette-target">{describeSource(connection.source)}</span>
                        )}
                      </li>
                    );
                  })}
                </ul>
              </li>
            ))}
          </ul>
        )}
        <div className="palette-footer muted small">
          <span>↑↓ scegli · ⏎ connetti · esc chiudi</span>
          <span className="spacer" />
          <button onClick={onManage}>Gestisci connessioni…</button>
        </div>
      </div>
    </div>
  );
}
