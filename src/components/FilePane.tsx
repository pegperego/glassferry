import { useVirtualizer } from "@tanstack/react-virtual";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type DragEvent,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from "react";
import { isDirLike, type DeleteMethod, type FileEntry } from "../api";
import { clickSelect, emptySelection, moveCursor, selectAll, toggleSort, type SortKey } from "../fileList";
import { formatPermissions, formatSize, formatTime } from "../format";
import { dragSourceSide, getDragPayload, setDragPayload, type DragPayload, type Side } from "../dnd";
import { joinPath, parentPath, pathSegments } from "../paths";
import type { PaneState } from "../usePane";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { ConfirmDialog, PromptDialog, nameError } from "./Dialogs";

const ROW_HEIGHT = 24;
const PAGE_ROWS = 20;
const CONFIRM_LIST_MAX = 8;

/** File operations within this pane's side. */
export interface PaneOps {
  mkdir: (dir: string, name: string) => Promise<void>;
  rename: (dir: string, from: string, to: string) => Promise<void>;
  move: (srcDir: string, names: string[], dstDir: string) => Promise<void>;
  remove: (dir: string, names: string[]) => Promise<DeleteMethod>;
  /** Deletes are permanent (remote) rather than moved to the Trash (local). */
  permanentDelete: boolean;
  /** How a delete is carried out, shown in the confirmation. */
  deleteHint: string;
}

interface Props {
  pane: PaneState;
  title: ReactNode;
  /** Extra header buttons (e.g. disconnect). */
  actions?: ReactNode;
  /** Informational line under the header. */
  info?: ReactNode;
  active: boolean;
  onActivate: () => void;
  side: Side;
  ops: PaneOps;
  /** Copy to the other pane (button and F5); absent when not possible. */
  transfer?: { label: string; title: string; run: (names: string[]) => void };
  /** Files dragged from the other pane, dropped into `targetDir`. */
  onDropFiles?: (payload: DragPayload, targetDir: string) => void;
}

type Dialog =
  | { kind: "mkdir" }
  | { kind: "move"; names: string[] }
  | { kind: "delete"; names: string[] };

export function FilePane(props: Props) {
  const { pane, title, actions, info, active, onActivate, side, ops, transfer, onDropFiles } = props;
  const { path, visible, selection, sort } = pane;
  const scrollRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const names = useMemo(() => visible.map((e) => e.name), [visible]);
  /** Drop target while dragging: a folder row name, "" for the pane itself. */
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; entry: FileEntry | null } | null>(null);
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [opError, setOpError] = useState<string | null>(null);

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

  useEffect(() => setOpError(null), [path]);

  const selectionOrCursor = () =>
    selection.selected.size > 0
      ? names.filter((n) => selection.selected.has(n))
      : selection.cursor
        ? [selection.cursor]
        : [];

  /** Runs an operation with a busy label, then refreshes (focusing `focus`). */
  const run = async (label: string, action: () => Promise<unknown>, focus?: string) => {
    setBusy(label);
    setOpError(null);
    try {
      await action();
    } catch (e) {
      setOpError(String(e));
    } finally {
      setBusy(null);
      await pane.refresh(focus);
      listRef.current?.focus();
    }
  };

  const open = (entry: FileEntry) => {
    if (path !== null && isDirLike(entry)) void pane.navigate(joinPath(path, entry.name));
  };

  const goUp = () => {
    if (path === null || path === "/") return;
    const from = path.replace(/\/+$/, "").split("/").pop();
    void pane.navigate(parentPath(path), from);
  };

  const startRename = (name: string | null) => {
    if (name !== null) setRenaming(name);
  };

  const commitRename = (from: string, to: string) => {
    setRenaming(null);
    if (path === null || to === from || nameError(to)) {
      listRef.current?.focus();
      return;
    }
    void run("Rinomina…", () => ops.rename(path, from, to), to);
  };

  const askDelete = (chosen: string[]) => {
    if (chosen.length > 0) setDialog({ kind: "delete", names: chosen });
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (renaming !== null) return;
    const move = (delta: number) => pane.setSelection(moveCursor(selection, names, delta, e.shiftKey));
    const cursorEntry = visible.find((v) => v.name === selection.cursor);
    let handled = true;
    if (e.key === "ArrowDown" && e.metaKey) {
      if (cursorEntry) open(cursorEntry);
    } else if (e.key === "ArrowUp" && e.metaKey) goUp();
    else if (e.key === "Backspace" && e.metaKey) askDelete(selectionOrCursor());
    else if (e.key === "Delete") askDelete(selectionOrCursor());
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
    else if (e.key === "F2") startRename(selection.cursor);
    else if (e.key === "F7" || (e.key.toLowerCase() === "n" && e.metaKey && e.shiftKey)) setDialog({ kind: "mkdir" });
    else if (e.key === "F5" && transfer) {
      const chosen = selectionOrCursor();
      if (chosen.length > 0) transfer.run(chosen);
    } else handled = false;
    if (handled) e.preventDefault();
  };

  // ---- drag & drop: other pane = copy, folder in this pane = move ----

  const onDragStart = (e: DragEvent, entry: FileEntry) => {
    if (path === null || renaming !== null) return;
    // Dragging an unselected row drags just that row, like the Finder.
    const dragged = selection.selected.has(entry.name) ? names.filter((n) => selection.selected.has(n)) : [entry.name];
    setDragPayload(e.dataTransfer, { side, dir: path, names: dragged });
    e.dataTransfer.effectAllowed = "copyMove";
  };

  const onDragOver = (e: DragEvent, entry?: FileEntry) => {
    const source = dragSourceSide(e.dataTransfer);
    if (source === null) return;
    const intoFolder = entry !== undefined && isDirLike(entry);
    if (source === side && !intoFolder) return;
    if (source !== side && !onDropFiles) return;
    e.preventDefault();
    e.stopPropagation();
    e.dataTransfer.dropEffect = source === side ? "move" : "copy";
    setDropTarget(intoFolder ? entry.name : "");
  };

  const onDrop = (e: DragEvent, entry?: FileEntry) => {
    setDropTarget(null);
    const payload = getDragPayload(e.dataTransfer);
    if (!payload || path === null) return;
    e.preventDefault();
    e.stopPropagation();
    const targetDir = entry && isDirLike(entry) ? joinPath(path, entry.name) : path;
    if (payload.side !== side) {
      onDropFiles?.(payload, targetDir);
    } else if (entry && isDirLike(entry) && !payload.names.includes(entry.name) && payload.dir !== targetDir) {
      void run("Spostamento…", () => ops.move(payload.dir, payload.names, targetDir));
    }
  };

  // ---- context menu ----

  const onContextMenu = (e: MouseEvent, entry: FileEntry | null) => {
    e.preventDefault();
    e.stopPropagation();
    onActivate();
    if (entry && !selection.selected.has(entry.name)) {
      pane.setSelection({ selected: new Set([entry.name]), cursor: entry.name, anchor: entry.name });
    }
    setMenu({ x: e.clientX, y: e.clientY, entry });
  };

  const menuItems = (entry: FileEntry | null): MenuItem[] => {
    const chosen = entry ? (selection.selected.has(entry.name) ? selectionOrCursor() : [entry.name]) : [];
    const items: MenuItem[] = [];
    if (entry && isDirLike(entry) && chosen.length === 1) items.push({ label: "Apri", shortcut: "↩", onSelect: () => open(entry) });
    if (entry && transfer) items.push({ label: transfer.label, shortcut: "F5", onSelect: () => transfer.run(chosen) });
    if (entry) {
      items.push(
        { separator: true },
        { label: "Rinomina", shortcut: "F2", disabled: chosen.length !== 1, onSelect: () => startRename(entry.name) },
        { label: "Sposta in…", onSelect: () => setDialog({ kind: "move", names: chosen }) },
        { label: "Copia percorso", onSelect: () => path && void navigator.clipboard.writeText(joinPath(path, entry.name)) },
        { separator: true },
        {
          label: ops.permanentDelete ? "Elimina…" : "Sposta nel Cestino",
          shortcut: "⌘⌫",
          danger: true,
          onSelect: () => askDelete(chosen),
        },
        { separator: true },
      );
    }
    items.push(
      { label: "Nuova cartella…", shortcut: "⇧⌘N", onSelect: () => setDialog({ kind: "mkdir" }) },
      { label: "Aggiorna", onSelect: () => void pane.refresh() },
    );
    if (!entry && path) items.push({ label: "Copia percorso cartella", onSelect: () => void navigator.clipboard.writeText(path) });
    return items;
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
        <button title="Nuova cartella (⇧⌘N)" onClick={() => setDialog({ kind: "mkdir" })} disabled={path === null}>
          +📁
        </button>
        <button title="Aggiorna" onClick={() => void pane.refresh()} disabled={pane.loading}>
          ⟳
        </button>
        {transfer && (
          <button
            className="transfer-button"
            title={`${transfer.title} (F5)`}
            disabled={selectionOrCursor().length === 0}
            onClick={() => transfer.run(selectionOrCursor())}
          >
            {transfer.label}
          </button>
        )}
        {actions}
      </header>
      {info && <p className="pane-info">{info}</p>}
      {pane.error && <p className="error">{pane.error}</p>}
      {opError && (
        <p className="error op-error">
          {opError}
          <button className="inline" onClick={() => setOpError(null)}>
            ✕
          </button>
        </p>
      )}

      <div
        ref={listRef}
        className={dropTarget === "" ? "file-list drop-target" : "file-list"}
        tabIndex={0}
        onKeyDown={onKeyDown}
        onDragOver={(e) => onDragOver(e)}
        onDragLeave={(e) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDropTarget(null);
        }}
        onDrop={(e) => onDrop(e)}
        onContextMenu={(e) => onContextMenu(e, null)}
        data-pane={side}
      >
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
              if (dropTarget === entry.name) classes.push("drop-target");
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
                  onContextMenu={(e) => onContextMenu(e, entry)}
                  draggable={path !== null && renaming !== entry.name}
                  onDragStart={(e) => onDragStart(e, entry)}
                  onDragOver={(e) => onDragOver(e, entry)}
                  onDrop={(e) => onDrop(e, entry)}
                >
                  <span className="col-name" title={entry.name}>
                    <span className="icon">{icon(entry)}</span>
                    {renaming === entry.name ? (
                      <RenameInput name={entry.name} onDone={(to) => commitRename(entry.name, to ?? entry.name)} />
                    ) : (
                      entry.name
                    )}
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
        {busy ??
          (pane.loading
            ? "Caricamento…"
            : selectedFiles.length > 0
              ? `${selectedFiles.length} di ${visible.length} selezionati${selectedBytes ? ` · ${formatSize(selectedBytes)}` : ""}`
              : `${visible.length} elementi`)}
      </footer>

      {menu && <ContextMenu x={menu.x} y={menu.y} items={menuItems(menu.entry)} onClose={() => setMenu(null)} />}

      {dialog?.kind === "mkdir" && path !== null && (
        <PromptDialog
          title="Nuova cartella"
          label={`In ${path}`}
          initial="nuova cartella"
          confirmLabel="Crea"
          validate={(v) => nameError(v) ?? (names.includes(v) ? "Esiste già un elemento con questo nome" : null)}
          onCancel={() => setDialog(null)}
          onSubmit={(name) => {
            setDialog(null);
            void run("Creazione cartella…", () => ops.mkdir(path, name), name);
          }}
        />
      )}
      {dialog?.kind === "move" && path !== null && (
        <PromptDialog
          title={`Sposta ${describe(dialog.names)}`}
          label="Cartella di destinazione"
          initial={path}
          confirmLabel="Sposta"
          validate={(v) => (v.startsWith("/") ? (v === path ? "È la cartella attuale" : null) : "Serve un percorso assoluto")}
          onCancel={() => setDialog(null)}
          onSubmit={(dst) => {
            setDialog(null);
            void run("Spostamento…", () => ops.move(path, dialog.names, dst.replace(/(.)\/+$/, "$1")));
          }}
        />
      )}
      {dialog?.kind === "delete" && path !== null && (
        <ConfirmDialog
          title={ops.permanentDelete ? `Eliminare ${describe(dialog.names)}?` : `Spostare ${describe(dialog.names)} nel Cestino?`}
          confirmLabel={ops.permanentDelete ? "Elimina definitivamente" : "Sposta nel Cestino"}
          danger={ops.permanentDelete}
          onCancel={() => setDialog(null)}
          onConfirm={() => {
            const chosen = dialog.names;
            setDialog(null);
            void run(ops.permanentDelete ? "Eliminazione…" : "Spostamento nel Cestino…", () => ops.remove(path, chosen));
          }}
        >
          <ul className="confirm-list">
            {dialog.names.slice(0, CONFIRM_LIST_MAX).map((n) => (
              <li key={n}>{n}</li>
            ))}
            {dialog.names.length > CONFIRM_LIST_MAX && <li className="muted">e altri {dialog.names.length - CONFIRM_LIST_MAX}…</li>}
          </ul>
          <p className={ops.permanentDelete ? "error-text small" : "muted small"}>{ops.deleteHint}</p>
        </ConfirmDialog>
      )}
    </section>
  );
}

function RenameInput({ name, onDone }: { name: string; onDone: (value: string | null) => void }) {
  const done = useRef(false);
  const finish = (value: string | null) => {
    if (done.current) return;
    done.current = true;
    onDone(value);
  };
  return (
    <input
      className="rename-input"
      autoFocus
      defaultValue={name}
      onFocus={(e) => {
        // Select the base name, not the extension, like the Finder.
        const dot = name.lastIndexOf(".");
        e.target.setSelectionRange(0, dot > 0 ? dot : name.length);
      }}
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") finish(e.currentTarget.value);
        if (e.key === "Escape") finish(null);
      }}
      onBlur={(e) => finish(e.currentTarget.value)}
    />
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

function describe(names: string[]): string {
  return names.length === 1 ? `"${names[0]}"` : `${names.length} elementi`;
}

function icon(entry: FileEntry): string {
  if (entry.kind === "symlink") return "🔗";
  return isDirLike(entry) ? "📁" : "📄";
}
