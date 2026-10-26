import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FileEntry } from "./api";
import { emptySelection, pruneSelection, visibleEntries, type Selection, type Sort } from "./fileList";

export type Lister = (path: string) => Promise<FileEntry[]>;

export interface PaneState {
  path: string | null;
  entries: FileEntry[];
  /** Entries after hidden-file filter and sorting, as displayed. */
  visible: FileEntry[];
  loading: boolean;
  error: string | null;
  sort: Sort;
  showHidden: boolean;
  selection: Selection;
  setSort: (sort: Sort) => void;
  setShowHidden: (show: boolean) => void;
  setSelection: (selection: Selection) => void;
  /** Loads `path`; `focus` is the entry to put the cursor on (e.g. the dir we came from). */
  navigate: (path: string, focus?: string) => Promise<void>;
  refresh: () => Promise<void>;
}

export function usePane(list: Lister, initialPath: string | null): PaneState {
  const [path, setPath] = useState<string | null>(null);
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sort, setSort] = useState<Sort>({ key: "name", ascending: true });
  const [showHidden, setShowHidden] = useState(false);
  const [selection, setSelection] = useState<Selection>(emptySelection);
  // Only the latest request may update the state (fast navigation).
  const requestId = useRef(0);

  const load = useCallback(
    async (target: string, onLoaded: (entries: FileEntry[]) => void) => {
      const id = ++requestId.current;
      setLoading(true);
      setError(null);
      try {
        const result = await list(target);
        if (id !== requestId.current) return;
        setEntries(result);
        setPath(target);
        onLoaded(result);
      } catch (e) {
        if (id === requestId.current) setError(String(e));
      } finally {
        if (id === requestId.current) setLoading(false);
      }
    },
    [list],
  );

  const navigate = useCallback(
    (target: string, focus?: string) =>
      load(target, () =>
        setSelection(
          focus ? { selected: new Set([focus]), cursor: focus, anchor: focus } : emptySelection,
        ),
      ),
    [load],
  );

  const refresh = useCallback(async () => {
    if (path === null) return;
    await load(path, (result) =>
      setSelection((current) => pruneSelection(current, result.map((e) => e.name))),
    );
  }, [load, path]);

  useEffect(() => {
    if (initialPath !== null) void navigate(initialPath);
  }, [initialPath, navigate]);

  const visible = useMemo(() => visibleEntries(entries, sort, showHidden), [entries, sort, showHidden]);

  return {
    path,
    entries,
    visible,
    loading,
    error,
    sort,
    showHidden,
    selection,
    setSort,
    setShowHidden,
    setSelection,
    navigate,
    refresh,
  };
}
