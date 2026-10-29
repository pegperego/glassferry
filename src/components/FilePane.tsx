import { useVirtualizer } from "@tanstack/react-virtual";
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from "react";
import {
  isDirLike,
  type AttrChange,
  type DirSize,
  type SearchQuery,
  type SearchResult,
  type FileEntry, type PlanOutcome, type PlanPreview, type PlanRequest } from "../api";
import type { Protection } from "../connections";
import { clickSelect, emptySelection, moveCursor, selectAll, toggleSort, type SortKey } from "../fileList";
import { formatOwner, formatPermissions, formatSize, formatTime, ownerTitle } from "../format";
import type { DragPayload, Side } from "../dnd";
import { beginPointerDrag, registerDropZone, useDragState, type DropZone } from "../pointerDrag";
import { joinPath, parentPath, pathSegments } from "../paths";
import type { PaneState } from "../usePane";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { PromptDialog, nameError } from "./Dialogs";
import { PermissionsDialog } from "./PermissionsDialog";
import { SearchDialog } from "./SearchDialog";
import { PlanDialog } from "./PlanDialog";

const ROW_HEIGHT = 24;
const PAGE_ROWS = 20;
const CONFIRM_LIST_MAX = 8;

/** File operations within this pane's side. */
export interface PaneOps {
  mkdir: (dir: string, name: string) => Promise<void>;
  rename: (dir: string, from: string, to: string) => Promise<void>;
  move: (srcDir: string, names: string[], dstDir: string) => Promise<void>;
  /** Previewed operations: deletes always, renames and moves on protected connections. */
  plan: (request: PlanRequest) => Promise<PlanPreview>;
  execute: (planId: number) => Promise<PlanOutcome>;
  discard: (planId: number) => void;
  /** Deletes are permanent (remote) rather than moved to the Trash (local). */
  permanentDelete: boolean;
  /** Remote connection of the pane; null for the local pane. */
  protection: Protection | null;
  /** Opens the system terminal in a folder (⌥⌘T); null on SFTP-only accounts. */
  openTerminal: ((dir: string) => Promise<void>) | null;
  /** chmod / chown: preview (`dryRun`) or apply. */
  attrs: (dir: string, names: string[], change: AttrChange, dryRun: boolean) => Promise<string[]>;
  /** Owner and group can change (remote account with chown/chgrp). */
  canChown: boolean;
  dirSize: (dir: string, names: string[]) => Promise<DirSize[]>;
  search: (dir: string, query: SearchQuery, id: number) => Promise<SearchResult>;
  cancelSearch: (id: number) => void;
  /** Into a .tar.gz in the same folder; null without tar. */
  compress: ((dir: string, names: string[], archive: string) => Promise<void>) | null;
  /** For an archive name: extracts it into a new folder (resolves to its name); null when not possible. */
  extract: (name: string) => ((dir: string) => Promise<string>) | null;
}

/** Bookmarks of the pane's connection (or of local folders). */
export interface PaneBookmarks {
  list: { id: string; name: string; path: string }[];
  /** Adds the folder, or removes it when already marked. */
  toggle: (path: string) => void;
}

interface Props {
  pane: PaneState;
  title: ReactNode;
  /** Extra header buttons (e.g. disconnect). */
  actions?: ReactNode;
  /** Informational line under the header. */
  info?: ReactNode;
  /** Category color of a remote connection: top border of the pane. */
  color?: string;
  active: boolean;
  onActivate: () => void;
  side: Side;
  ops: PaneOps;
  /** Copy to the default other pane (button and F5); absent when not possible. */
  transfer?: { label: string; title: string; run: (names: string[]) => void };
  /** ★ menu and ⌘D; absent while the connections are loading. */
  bookmarks?: PaneBookmarks;
  /** Every copy destination: ⇧F5 menu and context menu. */
  transferTargets?: { label: string; run: (names: string[]) => void }[];
  /** Files dragged from another pane, dropped into `targetDir`. */
  onDropFiles?: (payload: DragPayload, targetDir: string) => void;
  /** Panes whose drags this pane accepts (besides itself, which moves). */
  acceptsFrom?: Side[];
  /** Double click / Enter on a file: edit (remote) or open (local). */
  openFile?: { label: string; run: (path: string) => void };
}

type Dialog =
  | { kind: "mkdir" }
  | { kind: "compress"; names: string[] }
  | { kind: "search" }
  | { kind: "permissions"; names: string[] }
  | { kind: "move"; names: string[] }
  | {
      kind: "plan";
      request: PlanRequest;
      title: string;
      confirmLabel: string;
      busyLabel: string;
      danger: boolean;
      /** Names listed above the preview. */
      names: string[];
      focus?: string;
      preview: PlanPreview | null;
      error: string | null;
    };

export function FilePane(props: Props) {
  const { pane, title, actions, info, color, active, onActivate, side, ops, transfer, onDropFiles, openFile, bookmarks } = props;
  const transferTargets = props.transferTargets ?? [];
  const acceptsFrom = props.acceptsFrom ?? [];
  const { path, visible, selection, sort } = pane;
  const scrollRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const names = useMemo(() => visible.map((e) => e.name), [visible]);
  /** Drop target while dragging: a folder row name, "" for the pane itself. */
  const drag = useDragState();
  const dropTarget = drag?.target?.side === side ? (drag.target.entry ?? "") : null;
  const [renaming, setRenaming] = useState<string | null>(null);
  /** Context menu; with `targets`, the ⇧F5 menu of copy destinations for those names. */
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    entry: FileEntry | null;
    targets?: string[];
    bookmarks?: boolean;
  } | null>(null);
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
  /** Folder sizes computed on request, for the folder shown: name → size, or "…" while computing. */
  const [dirSizes, setDirSizes] = useState<{ path: string | null; sizes: Map<string, DirSize | "…"> }>({
    path: null,
    sizes: new Map(),
  });
  const sizes = dirSizes.path === path ? dirSizes.sizes : new Map<string, DirSize | "…">();
  const computeSizes = (folders: string[]) => {
    if (path === null || folders.length === 0) return;
    const at = path;
    const update = (f: (sizes: Map<string, DirSize | "…">) => void) =>
      setDirSizes((current) => {
        const next = new Map(current.path === at ? current.sizes : []);
        f(next);
        return { path: at, sizes: next };
      });
    update((s) => folders.forEach((name) => s.set(name, "…")));
    ops.dirSize(at, folders).then(
      (result) => update((s) => result.forEach((size) => s.set(size.name, size))),
      (e) => {
        update((s) => folders.forEach((name) => s.delete(name)));
        setOpError(String(e));
      },
    );
  };

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
    if (path === null) return;
    if (isDirLike(entry)) void pane.navigate(joinPath(path, entry.name));
    else if (entry.kind === "file" || entry.kind === "symlink") openFile?.run(joinPath(path, entry.name));
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
    if (ops.protection?.protected) {
      startPlan({ kind: "rename", dir: path, from, to }, {
        title: `Rinominare ${from} in ${to}?`,
        confirmLabel: "Rinomina",
        busyLabel: "Rinomina…",
        danger: false,
        names: [],
        focus: to,
      });
    } else void run("Rinomina…", () => ops.rename(path, from, to), to);
  };

  /** Moves within this side; previewed and confirmed on protected connections. */
  const moveTo = (srcDir: string, moved: string[], dstDir: string) => {
    if (ops.protection?.protected) {
      startPlan({ kind: "move", srcDir, names: moved, dstDir }, {
        title: `Spostare ${describe(moved)} in ${dstDir}?`,
        confirmLabel: "Sposta",
        busyLabel: "Spostamento…",
        danger: false,
        names: moved,
      });
    } else void run("Spostamento…", () => ops.move(srcDir, moved, dstDir));
  };

  type PlanMeta = Omit<Extract<Dialog, { kind: "plan" }>, "kind" | "request" | "preview" | "error">;
  /** Opens the confirmation and fetches the preview of `request`. */
  const startPlan = (request: PlanRequest, meta: PlanMeta) => {
    setDialog({ kind: "plan", request, ...meta, preview: null, error: null });
    ops.plan(request).then(
      (preview) =>
        setDialog((d) => {
          if (d?.kind === "plan" && d.request === request) return { ...d, preview };
          ops.discard(preview.id); // closed meanwhile
          return d;
        }),
      (e) => setDialog((d) => (d?.kind === "plan" && d.request === request ? { ...d, error: String(e) } : d)),
    );
  };

  const askDelete = (chosen: string[]) => {
    if (chosen.length === 0 || path === null) return;
    startPlan({ kind: "delete", dir: path, names: chosen }, {
      title: ops.permanentDelete ? `Eliminare ${describe(chosen)}?` : `Spostare ${describe(chosen)} nel Cestino?`,
      confirmLabel: ops.permanentDelete ? "Elimina definitivamente" : "Sposta nel Cestino",
      busyLabel: ops.permanentDelete ? "Eliminazione…" : "Spostamento nel Cestino…",
      danger: ops.permanentDelete,
      names: chosen,
    });
  };

  const terminalAt = (dir: string | null) => {
    if (!dir || !ops.openTerminal) return;
    setOpError(null);
    ops.openTerminal(dir).catch((e) => setOpError(String(e)));
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
    // ⌥ changes e.key on macOS (⌥T = †): match the physical key.
    else if (e.code === "KeyT" && e.metaKey && e.altKey) terminalAt(path);
    else if (e.key.toLowerCase() === "d" && e.metaKey && bookmarks && path) bookmarks.toggle(path);
    else if (e.key.toLowerCase() === "f" && e.metaKey && path) setDialog({ kind: "search" });
    else if (e.key === "F5" && e.shiftKey && transferTargets.length > 0) {
      const chosen = selectionOrCursor();
      const rect = listRef.current?.getBoundingClientRect();
      if (chosen.length > 0 && rect) setMenu({ x: rect.left + 24, y: rect.top + 8, entry: null, targets: chosen });
    } else if (e.key === "F5" && transfer) {
      const chosen = selectionOrCursor();
      if (chosen.length > 0) transfer.run(chosen);
    } else handled = false;
    if (handled) e.preventDefault();
  };

  // ---- drag & drop: other pane = copy, folder in this pane = move ----

  const dragPayload = (entry: FileEntry): DragPayload | null => {
    if (path === null || renaming !== null) return null;
    // Dragging an unselected row drags just that row, like the Finder.
    const dragged = selection.selected.has(entry.name) ? names.filter((n) => selection.selected.has(n)) : [entry.name];
    return { side, dir: path, names: dragged };
  };

  const folderPath = (entry: string | null) => {
    const found = entry === null ? undefined : visible.find((v) => v.name === entry);
    return found && isDirLike(found) && path !== null ? joinPath(path, found.name) : null;
  };

  const zone: DropZone = {
    accepts: (payload, entry) => {
      if (path === null) return false;
      const folder = folderPath(entry);
      if (payload.side === side) return folder !== null && !payload.names.includes(entry!) && payload.dir !== folder;
      return onDropFiles !== undefined && acceptsFrom.includes(payload.side);
    },
    drop: (payload, entry) => {
      if (path === null) return;
      const targetDir = folderPath(entry) ?? path;
      if (payload.side !== side) onDropFiles?.(payload, targetDir);
      else moveTo(payload.dir, payload.names, targetDir);
    },
  };
  // The registered zone always calls the latest render's handlers.
  const zoneRef = useRef(zone);
  zoneRef.current = zone;
  useEffect(
    () =>
      registerDropZone(side, {
        accepts: (payload, entry) => zoneRef.current.accepts(payload, entry),
        drop: (payload, entry) => zoneRef.current.drop(payload, entry),
      }),
    [side],
  );

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
    const chosenDirs = chosen.filter((name) => visible.some((v) => v.name === name && v.kind === "dir"));
    const extractor = entry && chosen.length === 1 && entry.kind === "file" && isArchive(entry.name) ? ops.extract(entry.name) : null;
    // The folder's name is known only at the end: the cursor goes there.
    const extractOne =
      extractor && path !== null
        ? () => {
            setBusy(`Estrazione di ${entry!.name}…`);
            setOpError(null);
            extractor(path)
              .then((folder) => pane.refresh(folder), (e) => setOpError(String(e)))
              .finally(() => {
                setBusy(null);
                listRef.current?.focus();
              });
          }
        : null;
    const items: MenuItem[] = [];
    if (entry && isDirLike(entry) && chosen.length === 1) items.push({ label: "Apri", shortcut: "↩", onSelect: () => open(entry) });
    if (entry && isDirLike(entry) && chosen.length === 1 && ops.openTerminal && path)
      items.push({ label: "Apri nel terminale", onSelect: () => terminalAt(joinPath(path, entry.name)) });
    if (entry && !isDirLike(entry) && openFile && chosen.length === 1)
      items.push({ label: openFile.label, shortcut: "↩", onSelect: () => open(entry) });
    if (entry && transfer) items.push({ label: transfer.label, shortcut: "F5", onSelect: () => transfer.run(chosen) });
    if (entry && transferTargets.length > 1)
      for (const target of transferTargets) items.push({ label: target.label, onSelect: () => target.run(chosen) });
    if (entry) {
      items.push(
        { separator: true },
        { label: "Rinomina", shortcut: "F2", disabled: chosen.length !== 1, onSelect: () => startRename(entry.name) },
        { label: "Sposta in…", onSelect: () => setDialog({ kind: "move", names: chosen }) },
        { label: "Permessi…", onSelect: () => setDialog({ kind: "permissions", names: chosen }) },
        ...(ops.compress ? [{ label: "Comprimi in .tar.gz…", onSelect: () => setDialog({ kind: "compress", names: chosen }) }] : []),
        ...(extractOne ? [{ label: "Estrai qui", onSelect: extractOne }] : []),
        ...(chosenDirs.length > 0
          ? [{ label: chosenDirs.length === 1 ? "Calcola dimensione" : `Calcola dimensione (${chosenDirs.length} cartelle)`, onSelect: () => computeSizes(chosenDirs) }]
          : []),
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
    if (!entry && path && ops.openTerminal)
      items.push({ label: "Apri nel terminale", shortcut: "⌥⌘T", onSelect: () => terminalAt(path) });
    if (!entry && path) items.push({ label: "Copia percorso cartella", onSelect: () => void navigator.clipboard.writeText(path) });
    return items;
  };

  const marked = bookmarks !== undefined && path !== null && bookmarks.list.some((b) => b.path === path.replace(/(.)\/+$/, "$1"));

  const bookmarkItems = (): MenuItem[] => {
    if (!bookmarks || path === null) return [];
    const items: MenuItem[] = [
      {
        label: marked ? "Togli questa cartella dai segnalibri" : "Aggiungi questa cartella ai segnalibri",
        shortcut: "⌘D",
        onSelect: () => bookmarks.toggle(path),
      },
    ];
    if (bookmarks.list.length > 0) items.push({ separator: true });
    for (const b of bookmarks.list) {
      items.push({ label: b.name === b.path ? b.path : `${b.name}  ·  ${b.path}`, onSelect: () => void pane.navigate(b.path) });
    }
    return items;
  };

  const selectedFiles = visible.filter((v) => selection.selected.has(v.name));
  const selectedBytes = selectedFiles.reduce((sum, v) => sum + (isDirLike(v) ? 0 : (v.size ?? 0)), 0);

  return (
    <section
      className={`pane${active ? " active" : ""}${color ? " colored" : ""}`}
      data-pane={side}
      style={color ? ({ "--pane-color": color } as CSSProperties) : undefined}
      onFocus={onActivate}
      onMouseDown={onActivate}
    >
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
        {bookmarks && (
          <button
            title="Segnalibri (⌘D aggiunge o toglie questa cartella)"
            className={marked ? "toggle on" : "toggle"}
            disabled={path === null}
            onClick={(e) => {
              const rect = e.currentTarget.getBoundingClientRect();
              setMenu({ x: rect.left, y: rect.bottom + 2, entry: null, bookmarks: true });
            }}
          >
            {marked ? "★" : "☆"}
          </button>
        )}
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
        <button title="Cerca in questa cartella (⌘F)" disabled={path === null} onClick={() => setDialog({ kind: "search" })}>
          ⌕
        </button>
        {ops.openTerminal && (
          <button title="Apri nel terminale (⌥⌘T)" className="mono" disabled={path === null} onClick={() => terminalAt(path)}>
            &gt;_
          </button>
        )}
        {transfer && (
          <button
            className="transfer-button"
            title={`${transfer.title} (F5${transferTargets.length > 1 ? ", ⇧F5 per scegliere" : ""})`}
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
        onContextMenu={(e) => onContextMenu(e, null)}
      >
        <div className="file-row file-head">
          <SortHeader label="Nome" column="name" sort={sort} onSort={pane.setSort} />
          <SortHeader label="Dimensione" column="size" sort={sort} onSort={pane.setSort} className="num" />
          <SortHeader label="Modificato" column="modified" sort={sort} onSort={pane.setSort} />
          <span className="col-perm">Permessi</span>
          <span className="col-owner">Proprietario</span>
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
                  data-entry={entry.name}
                  data-dir={isDirLike(entry) ? "1" : undefined}
                  onPointerDown={(e) => {
                    if (renaming !== entry.name) beginPointerDrag(e, () => dragPayload(entry));
                  }}
                >
                  <span className="col-name" title={entry.name}>
                    <span className="icon">{icon(entry)}</span>
                    {renaming === entry.name ? (
                      <RenameInput name={entry.name} onDone={(to) => commitRename(entry.name, to ?? entry.name)} />
                    ) : (
                      entry.name
                    )}
                  </span>
                  {isDirLike(entry) ? <FolderSize size={sizes.get(entry.name)} /> : <span className="num">{formatSize(entry.size)}</span>}
                  <span>{formatTime(entry.modified)}</span>
                  <span className="col-perm mono">{formatPermissions(entry.permissions)}</span>
                  <span className="col-owner mono" title={ownerTitle(entry)}>
                    {formatOwner(entry)}
                  </span>
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

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={
            menu.bookmarks
              ? bookmarkItems()
              : menu.targets
                ? transferTargets.map((t) => ({ label: t.label, onSelect: () => t.run(menu.targets!) }))
                : menuItems(menu.entry)
          }
          onClose={() => setMenu(null)}
        />
      )}

      {dialog?.kind === "search" && path !== null && (
        <SearchDialog
          dir={path}
          search={ops.search}
          cancel={ops.cancelSearch}
          onClose={() => setDialog(null)}
          onOpen={(found, isDir) => {
            setDialog(null);
            // A folder opens; a file shows in its folder, under the cursor.
            void (isDir ? pane.navigate(found) : pane.navigate(parentPath(found), found.slice(found.lastIndexOf("/") + 1)));
            listRef.current?.focus();
          }}
        />
      )}
      {dialog?.kind === "permissions" && path !== null && (
        <PermissionsDialog
          dir={path}
          entries={visible.filter((v) => dialog.names.includes(v.name))}
          attrs={ops.attrs}
          canChown={ops.canChown}
          protection={ops.protection}
          onCancel={() => setDialog(null)}
          onDone={() => {
            setDialog(null);
            void pane.refresh();
            listRef.current?.focus();
          }}
        />
      )}
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
      {dialog?.kind === "compress" && path !== null && ops.compress && (
        <PromptDialog
          title={`Comprimi ${describe(dialog.names)}`}
          label={`Archivio in ${path}`}
          initial={dialog.names.length === 1 ? `${dialog.names[0]}.tar.gz` : "archivio.tar.gz"}
          confirmLabel="Comprimi"
          validate={(v) =>
            nameError(v) ??
            (!/\.(tar\.gz|tgz)$/i.test(v) ? "Il nome deve finire con .tar.gz" : names.includes(v) ? "Esiste già un elemento con questo nome" : null)
          }
          onCancel={() => setDialog(null)}
          onSubmit={(archive) => {
            const compress = ops.compress!;
            setDialog(null);
            void run(`Compressione in ${archive}…`, () => compress(path, dialog.names, archive), archive);
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
            moveTo(path, dialog.names, dst.replace(/(.)\/+$/, "$1"));
          }}
        />
      )}
      {dialog?.kind === "plan" && (
        <PlanDialog
          title={dialog.title}
          preview={dialog.preview}
          error={dialog.error}
          protection={ops.protection}
          confirmLabel={dialog.confirmLabel}
          danger={dialog.danger}
          onCancel={() => {
            if (dialog.preview) ops.discard(dialog.preview.id);
            setDialog(null);
            listRef.current?.focus();
          }}
          onConfirm={() => {
            const { preview, busyLabel, focus } = dialog;
            setDialog(null);
            if (preview) void run(busyLabel, () => ops.execute(preview.id), focus);
          }}
        >
          {dialog.names.length > 0 && (
            <ul className="confirm-list">
              {dialog.names.slice(0, CONFIRM_LIST_MAX).map((n) => (
                <li key={n}>{n}</li>
              ))}
              {dialog.names.length > CONFIRM_LIST_MAX && (
                <li className="muted">e altri {dialog.names.length - CONFIRM_LIST_MAX}…</li>
              )}
            </ul>
          )}
        </PlanDialog>
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

/** Size of a folder computed on request: `~` for disk usage (du -k), `+` when some content was unreadable. */
function FolderSize({ size }: { size: DirSize | "…" | undefined }) {
  if (size === undefined) return <span className="num" />;
  if (size === "…") return <span className="num muted">…</span>;
  const title = [
    size.approx ? "spazio occupato su disco (du)" : "somma delle dimensioni dei file",
    size.partial ? "alcune cartelle non erano leggibili" : "",
  ]
    .filter(Boolean)
    .join("; ");
  return (
    <span className="num folder-size" title={title}>
      {size.approx ? "~" : ""}
      {formatSize(size.bytes)}
      {size.partial ? "+" : ""}
    </span>
  );
}

const ARCHIVE = /\.(tar|tar\.gz|tgz|tar\.bz2|tbz2|tar\.xz|txz|tar\.zst|zip)$/i;

/** Mirrors `archive::split` in the backend. */
function isArchive(name: string): boolean {
  return ARCHIVE.test(name) && name.replace(ARCHIVE, "") !== "";
}
