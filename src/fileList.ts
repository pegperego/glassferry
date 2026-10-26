// Pure list logic shared by both panes: visibility, sorting, selection.

import { isDirLike, type FileEntry } from "./api";

export type SortKey = "name" | "size" | "modified";

export interface Sort {
  key: SortKey;
  ascending: boolean;
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** Directories always come first; within each group the chosen key applies. */
export function visibleEntries(entries: FileEntry[], sort: Sort, showHidden: boolean): FileEntry[] {
  const direction = sort.ascending ? 1 : -1;
  return entries
    .filter((e) => showHidden || !e.name.startsWith("."))
    .sort((a, b) => {
      const dirFirst = Number(isDirLike(b)) - Number(isDirLike(a));
      if (dirFirst !== 0) return dirFirst;
      let result = 0;
      if (sort.key === "size") result = (a.size ?? 0) - (b.size ?? 0);
      else if (sort.key === "modified") result = (a.modified ?? 0) - (b.modified ?? 0);
      if (result === 0) result = collator.compare(a.name, b.name);
      return result * direction;
    });
}

export function toggleSort(current: Sort, key: SortKey): Sort {
  return current.key === key ? { key, ascending: !current.ascending } : { key, ascending: true };
}

export interface Selection {
  selected: ReadonlySet<string>;
  /** Row with keyboard focus. */
  cursor: string | null;
  /** Fixed end of a shift-range. */
  anchor: string | null;
}

export const emptySelection: Selection = { selected: new Set(), cursor: null, anchor: null };

function range(names: string[], from: string | null, to: string): Set<string> {
  const start = from === null ? -1 : names.indexOf(from);
  const end = names.indexOf(to);
  if (start < 0) return new Set([to]);
  const [lo, hi] = start <= end ? [start, end] : [end, start];
  return new Set(names.slice(lo, hi + 1));
}

/** Mouse click with Finder semantics: plain, cmd (toggle), shift (range). */
export function clickSelect(
  selection: Selection,
  names: string[],
  name: string,
  modifiers: { toggle: boolean; extend: boolean },
): Selection {
  if (modifiers.extend) {
    return { selected: range(names, selection.anchor, name), cursor: name, anchor: selection.anchor ?? name };
  }
  if (modifiers.toggle) {
    const selected = new Set(selection.selected);
    if (selected.has(name)) selected.delete(name);
    else selected.add(name);
    return { selected, cursor: name, anchor: name };
  }
  return { selected: new Set([name]), cursor: name, anchor: name };
}

/** Arrow keys / Home / End. `delta` is clamped to the list bounds. */
export function moveCursor(selection: Selection, names: string[], delta: number, extend: boolean): Selection {
  if (names.length === 0) return emptySelection;
  const current = selection.cursor === null ? -1 : names.indexOf(selection.cursor);
  const start = current < 0 ? (delta > 0 ? -1 : names.length) : current;
  const next = names[Math.max(0, Math.min(names.length - 1, start + delta))];
  if (extend) {
    const anchor = selection.anchor ?? selection.cursor ?? next;
    return { selected: range(names, anchor, next), cursor: next, anchor };
  }
  return { selected: new Set([next]), cursor: next, anchor: next };
}

export function selectAll(names: string[]): Selection {
  return { selected: new Set(names), cursor: names[0] ?? null, anchor: names[0] ?? null };
}

/** Drops names that no longer exist (after a refresh). */
export function pruneSelection(selection: Selection, names: string[]): Selection {
  const present = new Set(names);
  const selected = new Set([...selection.selected].filter((n) => present.has(n)));
  return {
    selected,
    cursor: selection.cursor !== null && present.has(selection.cursor) ? selection.cursor : null,
    anchor: selection.anchor !== null && present.has(selection.anchor) ? selection.anchor : null,
  };
}
