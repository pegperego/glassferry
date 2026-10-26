import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useMemo, useRef, type KeyboardEvent, type ReactNode } from "react";
import { isDirLike, type FileEntry } from "../api";
import {
  clickSelect,
  emptySelection,
  moveCursor,
  selectAll,
  toggleSort,
  type SortKey,
} from "../fileList";
import { formatPermissions, formatSize, formatTime } from "../format";
import { joinPath, parentPath, pathSegments } from "../paths";
import type { PaneState } from "../usePane";

const ROW_HEIGHT = 24;
const PAGE_ROWS = 20;

interface Props {
  pane: PaneState;
  title: ReactNode;
  /** Extra header buttons (e.g. disconnect). */
  actions?: ReactNode;
  /** Informational line under the header. */
  info?: ReactNode;
  active: boolean;
  onActivate: () => void;
}

export function FilePane({ pane, title, actions, info, active, onActivate }: Props) {
  const { path, visible, selection, sort } = pane;
  const scrollRef = useRef<HTMLDivElement>(null);
  const names = useMemo(() => visible.map((e) => e.name), [visible]);

  const virtualizer = useVirtualizer({
    count: visible.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
  });

  // Keep the keyboard cursor in view.
  useEffect(() => {
    if (selection.cursor === null) return;
    const index = names.indexOf(selection.cursor);
    if (index >= 0) virtualizer.scrollToIndex(index, { align: "auto" });
  }, [selection.cursor, names, virtualizer]);

  const open = (entry: FileEntry) => {
    if (path !== null && isDirLike(entry)) void pane.navigate(joinPath(path, entry.name));
  };

  const goUp = () => {
    if (path === null || path === "/") return;
    const from = path.replace(/\/+$/, "").split("/").pop();
    void pane.navigate(parentPath(path), from);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const move = (delta: number) => pane.setSelection(moveCursor(selection, names, delta, e.shiftKey));
    const cursorEntry = visible.find((v) => v.name === selection.cursor);
    let handled = true;
    if (e.key === "ArrowDown" && e.metaKey) {
      if (cursorEntry) open(cursorEntry);
    } else if (e.key === "ArrowUp" && e.metaKey) goUp();
    else if (e.key === "ArrowDown") move(1);
    else if (e.key === "ArrowUp") move(-1);
    else if (e.key === "PageDown") move(PAGE_ROWS);
    else if (e.key === "PageUp") move(-PAGE_ROWS);
    else if (e.key === "Home") move(-names.length);
    else if (e.key === "End") move(names.length);
    else if (e.key === "Enter" && cursorEntry) open(cursorEntry);
    else if (e.key === "Backspace") goUp();
    else if (e.key === "Escape") pane.setSelection(emptySelection);
    else if (e.key === "a" && e.metaKey) pane.setSelection(selectAll(names));
    else if (e.key === "." && e.metaKey && e.shiftKey) pane.setShowHidden(!pane.showHidden);
    else handled = false;
    if (handled) e.preventDefault();
  };

  const selectedFiles = visible.filter((v) => selection.selected.has(v.name));
  const selectedBytes = selectedFiles.reduce((sum, v) => sum + (isDirLike(v) ? 0 : (v.size ?? 0)), 0);

  return (
    <section className={active ? "pane active" : "pane"} onFocus={onActivate} onMouseDown={onActivate}>
      <header className="pane-header">
        <strong className="pane-title">{title}</strong>
        <nav className="breadcrumb">
          {path !== null &&
            pathSegments(path).map((segment) => (
              <button key={segment.path} onClick={() => void pane.navigate(segment.path)}>
                {segment.name}
              </button>
            ))}
        </nav>
        <button
          title="Mostra file nascosti (⇧⌘.)"
          className={pane.showHidden ? "toggle on" : "toggle"}
          onClick={() => pane.setShowHidden(!pane.showHidden)}
        >
          .*
        </button>
        <button title="Aggiorna" onClick={() => void pane.refresh()} disabled={pane.loading}>
          ⟳
        </button>
        {actions}
      </header>
      {info && <p className="pane-info">{info}</p>}
      {pane.error && <p className="error">{pane.error}</p>}

      <div className="file-list" tabIndex={0} onKeyDown={onKeyDown}>
        <div className="file-row file-head">
          <SortHeader label="Nome" column="name" sort={sort} onSort={pane.setSort} />
          <SortHeader label="Dimensione" column="size" sort={sort} onSort={pane.setSort} className="num" />
          <SortHeader label="Modificato" column="modified" sort={sort} onSort={pane.setSort} />
          <span className="col-perm">Permessi</span>
        </div>
        <div className="file-scroll" ref={scrollRef}>
          <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
            {virtualizer.getVirtualItems().map((item) => {
              const entry = visible[item.index];
              const classes = ["file-row"];
              if (selection.selected.has(entry.name)) classes.push("selected");
              if (selection.cursor === entry.name) classes.push("cursor");
              return (
                <div
                  key={entry.name}
                  className={classes.join(" ")}
                  style={{ transform: `translateY(${item.start}px)`, height: ROW_HEIGHT }}
                  onClick={(e) =>
                    pane.setSelection(
                      clickSelect(selection, names, entry.name, { toggle: e.metaKey, extend: e.shiftKey }),
                    )
                  }
                  onDoubleClick={() => open(entry)}
                >
                  <span className="col-name" title={entry.name}>
                    <span className="icon">{icon(entry)}</span>
                    {entry.name}
                  </span>
                  <span className="num">{isDirLike(entry) ? "" : formatSize(entry.size)}</span>
                  <span>{formatTime(entry.modified)}</span>
                  <span className="col-perm mono">{formatPermissions(entry.permissions)}</span>
                </div>
              );
            })}
          </div>
        </div>
      </div>

      <footer className="pane-footer">
        {pane.loading
          ? "Caricamento…"
          : selectedFiles.length > 0
            ? `${selectedFiles.length} di ${visible.length} selezionati${selectedBytes ? ` · ${formatSize(selectedBytes)}` : ""}`
            : `${visible.length} elementi`}
      </footer>
    </section>
  );
}

function SortHeader(props: {
  label: string;
  column: SortKey;
  sort: { key: SortKey; ascending: boolean };
  onSort: (sort: { key: SortKey; ascending: boolean }) => void;
  className?: string;
}) {
  const arrow = props.sort.key === props.column ? (props.sort.ascending ? " ▲" : " ▼") : "";
  return (
    <button className={`sort ${props.className ?? ""}`} onClick={() => props.onSort(toggleSort(props.sort, props.column))}>
      {props.label}
      {arrow}
    </button>
  );
}

function icon(entry: FileEntry): string {
  if (entry.kind === "symlink") return "🔗";
  return isDirLike(entry) ? "📁" : "📄";
}
