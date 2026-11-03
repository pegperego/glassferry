// Lines of a followed file (tail): bounded buffer, regex filter, highlights.
// Pure, tested; the dialog is TailDialog.

/** Lines kept in memory; older ones are dropped. */
export const MAX_LINES = 10_000;
/** Lines shown when a file is opened. */
export const INITIAL_LINES = 500;

export interface TailLine {
  /** Increasing, stable across trimming (React key, virtualizer). */
  id: number;
  text: string;
  /** A notice from the app (truncated, rotated…), not a line of the file. */
  notice: boolean;
}

export interface TailBuffer {
  lines: TailLine[];
  nextId: number;
  /** Lines dropped so far because of `MAX_LINES`. */
  dropped: number;
}

export const emptyBuffer = (): TailBuffer => ({ lines: [], nextId: 1, dropped: 0 });

/** Appends lines (or one notice), dropping the oldest past `max`. */
export function append(buffer: TailBuffer, texts: string[], notice = false, max = MAX_LINES): TailBuffer {
  if (texts.length === 0) return buffer;
  let nextId = buffer.nextId;
  const added = texts.map((text) => ({ id: nextId++, text, notice }));
  const all = buffer.lines.concat(added);
  const extra = Math.max(0, all.length - max);
  return { lines: extra > 0 ? all.slice(extra) : all, nextId, dropped: buffer.dropped + extra };
}

export type Filter = { regex: RegExp | null; error: string | null };

/** The filter typed by the user: a regular expression; empty = no filter. */
export function compileFilter(text: string, caseSensitive: boolean): Filter {
  if (text === "") return { regex: null, error: null };
  try {
    return { regex: new RegExp(text, caseSensitive ? "g" : "gi"), error: null };
  } catch (e) {
    return { regex: null, error: e instanceof Error ? e.message : String(e) };
  }
}

/** Lines matching the filter; notices always stay. */
export function filterLines(lines: TailLine[], regex: RegExp | null): TailLine[] {
  if (!regex) return lines;
  return lines.filter((line) => {
    if (line.notice) return true;
    regex.lastIndex = 0;
    return regex.test(line.text);
  });
}

export interface Segment {
  text: string;
  match: boolean;
}

/** `text` split into matching and non-matching parts. */
export function highlight(text: string, regex: RegExp | null): Segment[] {
  if (!regex) return [{ text, match: false }];
  const segments: Segment[] = [];
  let last = 0;
  regex.lastIndex = 0;
  for (let m = regex.exec(text); m !== null; m = regex.exec(text)) {
    if (m[0] === "") {
      // An empty match (e.g. `a*`) would loop forever.
      regex.lastIndex++;
      continue;
    }
    if (m.index > last) segments.push({ text: text.slice(last, m.index), match: false });
    segments.push({ text: m[0], match: true });
    last = m.index + m[0].length;
  }
  if (last < text.length || segments.length === 0) segments.push({ text: text.slice(last), match: false });
  return segments;
}
