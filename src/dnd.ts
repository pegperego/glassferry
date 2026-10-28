// Drag & drop of file selections between panes (see pointerDrag.ts).

/** The local pane and the two remote slots. */
export type Side = "local" | "r0" | "r1";


export const remoteSide = (slot: number): Side => (slot === 0 ? "r0" : "r1");

/** Remote slot of a side, or null for the local pane. */
export const slotOf = (side: Side): number | null => (side === "local" ? null : side === "r0" ? 0 : 1);

export interface DragPayload {
  side: Side;
  dir: string;
  names: string[];
}

/** Groups absolute local paths by parent directory (Finder drops). */
export function groupByParent(paths: string[]): Map<string, string[]> {
  const groups = new Map<string, string[]>();
  for (const path of paths) {
    const trimmed = path.replace(/\/+$/, "");
    const index = trimmed.lastIndexOf("/");
    if (index < 0) continue;
    const dir = index === 0 ? "/" : trimmed.slice(0, index);
    const name = trimmed.slice(index + 1);
    groups.set(dir, [...(groups.get(dir) ?? []), name]);
  }
  return groups;
}
