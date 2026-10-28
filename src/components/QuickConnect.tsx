import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import type { Store } from "../api";
import { categoryOf, describeSource, groupByCategory, searchBookmarks, searchConnections } from "../connections";

interface Props {
  store: Store;
  onConnect: (connectionId: string) => void;
  /** A bookmark: that connection, at that folder. */
  onOpenBookmark: (connectionId: string, path: string) => void;
  onManage: () => void;
  onClose: () => void;
}

/**
 * "Nuovo collegamento" (⌘K): search a connection, Enter or double click connects.
 * A single click only moves the cursor: connecting on click closed the palette
 * under the pointer, so the second click of an instinctive double click landed
 * on the pane below and opened a folder or a file. While typing, matching
 * bookmarks come first.
 */
export function QuickConnect({ store, onConnect, onOpenBookmark, onManage, onClose }: Props) {
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);
  const matches = useMemo(() => searchConnections(store, query), [store, query]);
  const groups = useMemo(() => groupByCategory(store, matches), [store, matches]);
  const marks = useMemo(() => searchBookmarks(store, query), [store, query]);
  // Display order drives the keyboard cursor: bookmarks, then connections by category.
  const ordered = groups.flatMap((g) => g.items);
  const total = marks.length + ordered.length;
  const choose = (index: number) => {
    const mark = marks[index];
    if (mark) onOpenBookmark(mark.connection.id, mark.bookmark.path);
    else if (ordered[index - marks.length]) onConnect(ordered[index - marks.length].id);
  };
  const listRef = useRef<HTMLUListElement>(null);

  useEffect(() => setCursor(0), [query]);
  useEffect(() => {
    listRef.current?.querySelector(".cursor")?.scrollIntoView({ block: "nearest" });
  }, [cursor]);

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") onClose();
    else if (e.key === "ArrowDown") setCursor((c) => Math.min(c + 1, total - 1));
    else if (e.key === "ArrowUp") setCursor((c) => Math.max(c - 1, 0));
    else if (e.key === "Enter") choose(cursor);
    else return;
    e.preventDefault();
  };

  return (
    <div className="modal-backdrop top" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal palette" onKeyDown={onKey}>
        <input
          className="palette-input"
          autoFocus
          placeholder="Cerca per nome, host, alias, categoria o segnalibro…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        {store.connections.length === 0 ? (
          <p className="muted palette-empty">
            Nessuna connessione. Importale da ~/.ssh/config o creane una nuova in "Gestisci connessioni".
          </p>
        ) : total === 0 ? (
          <p className="muted palette-empty">Nessuna connessione corrisponde.</p>
        ) : (
          <ul className="palette-list" ref={listRef}>
            {marks.length > 0 && (
              <li>
                <div className="palette-group">★ Segnalibri</div>
                <ul>
                  {marks.map(({ bookmark, connection }, index) => (
                    <li
                      key={bookmark.id}
                      className={index === cursor ? "palette-item cursor" : "palette-item"}
                      onMouseMove={() => setCursor(index)}
                      onClick={() => setCursor(index)}
                      onDoubleClick={() => choose(index)}
                    >
                      <span className="category-bar" style={{ background: categoryOf(store, connection)?.color }} />
                      <span className="palette-name">{bookmark.name}</span>
                      <span className="muted palette-target">
                        {connection.name}:{bookmark.path}
                      </span>
                    </li>
                  ))}
                </ul>
              </li>
            )}
            {groups.map((group) => (
              <li key={group.category.id}>
                <div className="palette-group">
                  <span className="category-dot" style={{ background: group.category.color }} />
                  {group.category.name}
                  {group.category.protected && <span className="muted"> · protetta</span>}
                </div>
                <ul>
                  {group.items.map((connection) => {
                    const index = marks.length + ordered.indexOf(connection);
                    return (
                      <li
                        key={connection.id}
                        className={index === cursor ? "palette-item cursor" : "palette-item"}
                        onMouseMove={() => setCursor(index)}
                        onClick={() => setCursor(index)}
                        onDoubleClick={() => choose(index)}
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
          <span>↑↓ scegli · ⏎ o doppio clic connetti · esc chiudi</span>
          <span className="spacer" />
          <button onClick={onManage}>Gestisci connessioni…</button>
        </div>
      </div>
    </div>
  );
}
